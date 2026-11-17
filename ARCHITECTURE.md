# Architecture

Normative requirements live in `MASTER.md`. This file records the decisions
taken while implementing them, including the two places where the code
deliberately deviates.

## Modules

| Path | Responsibility |
|---|---|
| `src/main.rs` | CLI parsing, terminal lifecycle, error reporting |
| `src/app/` | Event loop, key routing, prompts, command dispatch |
| `src/state.rs` | `AppState`, ids, pane viewport state |
| `src/command.rs` | The complete mutation vocabulary |
| `src/editor/` | Document, undo history, grapheme/cell helpers |
| `src/workspace/` | Layout tree, workspace manager |
| `src/explorer/` | Lazy tree, `.gitignore` filtering, filesystem ops |
| `src/input/` | Key combinations, binding presets, prompt state |
| `src/ui/` | Read-only rendering, rect computation |
| `src/config/` | TOML loading with per-section fallback |
| `src/terminal.rs` | Raw mode, bracketed paste, mouse, cleanup |
| `src/clipboard.rs` | Internal clipboard plus OSC 52 |

`main.rs` wires them; no module owns more than one concern.

## Decisions

**Document, pane, and workspace stay separate.** A document owns the rope and
history; a pane owns cursor, selection, and scroll; a workspace owns a layout
tree. Two panes can show one buffer, which is why cursor state is never stored
on `Document`.

**Split axes are `LeftRight` and `TopBottom`.** Ratatui's
`Direction::Horizontal` means left-to-right, while "horizontal split" is
commonly read as top-to-bottom. Renaming the axes removes the translation.

**Splits store weights, one per child.** A `u16` per child in `1..=100`,
normalized for rendering. A single ratio cannot describe three children.

**Rectangles are computed in this crate, not by `Layout` constraints.**
`Layout` cannot guarantee the minimum pane size while distributing leftover
cells, so `workspace::layout::split_area` performs the split and Ratatui renders
into the result. This is the one deliberate deviation from "use Ratatui Layout
for split calculation", traded for the mandatory minimum-size rule.

**Minimums are enforced, not hoped for.** `MIN_PANE_ROWS = 4`,
`MIN_PANE_COLS = 10`, viewport arithmetic is saturating everywhere, and a split
is rejected when the computed rects would be smaller.

**Layout changes refresh the rect cache immediately.** `refresh_layout_rects`
runs after split, close, and workspace switches, so focus movement and mouse
hit-testing never read a stale map.

**Fullscreen is a view filter, not a layout mutation.** `AppState::fullscreen`
holds one pane id; `ui::layout_rects` returns only that pane's rect for the whole
pane area. The layout tree, split ratios, and sibling scroll positions survive
untouched, and closing the pane or switching workspaces clears the flag.

**Moving a pane swaps leaf ids, not tree nodes.** `LayoutNode::swap_panes`
exchanges the two `PaneId`s in place, so the tree shape and weights are
untouched, no minimum-size invariant can break, and the moved pane keeps its
cursor, tabs, and scroll while inheriting the target rectangle. Neighbours are
found from the rect cache by edge overlap, the same geometry used for focus.

**A displaced focus falls back to the remembered pane.** When the explorer has
focus, `focused_or_first_pane` uses `Workspace::focus` (the last edited pane)
before any leaf, so opening a file from the explorer no longer jumps to the
first pane. That was the "two files always land in one pane" bug.

**Grab mode is an explicit mode because the chord is unavailable.** Terminals do
not reliably deliver `Ctrl+Shift`/`Ctrl+Alt` arrows across emulators, so `F10`
toggles a mode where plain arrows move the focused pane; `Esc` or `F10` exits.
The mode is rendered in the status bar and swallows other keys rather than
letting a stray keystroke edit text.

**Tabbed containers are a split mode, not a new node type.** `SplitMode::Tabbed`
keeps the same children and weights but shows one child and reserves
`TAB_BAR_ROWS` for the strip. Consequences that fall out of that choice: minimum
size becomes `max(child)` plus one row instead of a sum, separators are not
reported for tabbed splits, `split_pane` appends a tab (bounded by `MAX_TABS`),
and `cycle_tab` walks to the innermost tabbed ancestor before deciding. Tab strips
are computed into `UiState::tab_bars` together with pane rects, so rendering and
mouse hit-testing read one map.

**Pane geometry and tab strips are refreshed together.** `ui::refresh_panes`
recomputes both from the pane area; `App::refresh_layout_rects` delegates to it.
That keeps keyboard commands and mouse hit-testing consistent without waiting for
the next draw.

**Pane and explorer geometry are shared between rendering and input.**
`ui::pane::text_geometry` and `ui::overlay::explorer_geometry` compute the inner
area, tab row, gutter, and scroll offset once; the renderer and mouse handlers
both call them. A click therefore lands on the cell it visually points at, and a
layout change cannot desynchronize hit-testing from drawing.

**Wheel scrolling moves the viewport, not the cursor.** `scroll_pane` clamps
`scroll_top` to `line_count - rows` and `scroll_left` to the widest visible line.
The next keyboard motion calls `ensure_cursor_visible`, which pulls the viewport
back to the cursor; that split is intentional so scrolling does not fight cursor
placement.

**Double-click is a timestamp, not a window-system gesture.** `App` keeps the last
explorer row and its `Instant`; a second click on the same row within 400 ms opens
the entry. This keeps the behavior testable without a real terminal.

**Key bindings are data.** `KeyMap` holds three scopes (`global`, `editing`,
`explorer`). Resolution is prompt, focused scope, global. Startup probes
keyboard enhancement support, falls back to the `safe` preset, and reports
conflicts instead of silently dropping a binding.

**Undo is an operation log.** Ropey has no history. Each `Edit` records the
char index and the text it inserted or removed; `Batch` adds cursor and scroll
to restore and coalesces typing within 300 ms, breaking the run on a newline,
cursor move, or non-insert operation.

**Cursor positions are char indices.** Byte offsets and display columns are
derived at the boundary by `editor::text`, which uses `unicode-segmentation` for
graphemes and `unicode-width` for cells.

**Prompts are one subsystem.** Quick open, find, replace, save-as, new file,
rename, delete, discard, and go-to-line all use `PromptState`. While a prompt is
active it consumes every key except the quit binding, so a dialog cannot leak
input into a document.

**Rendering is read-only.** `ui::render` takes `&AppState`. The only mutation
before a draw is `compute_layout`, which fills the rect cache.

**Terminal cleanup does not depend on a clean exit.** Ratatui installs a panic
hook; `terminal.rs` chains its own hook to leave bracketed paste, mouse capture,
and keyboard-enhancement flags. `signal-hook` restores the terminal on SIGINT,
SIGTERM, and SIGHUP. `signal-hook` is the single dependency beyond the stack
listed in `MASTER.md`; it exists because a signal-killed editor otherwise leaves
the terminal unusable.

**Dirty documents are never dropped implicitly.** Closing a dirty document opens
a discard prompt. Closing a pane keeps its dirty documents registered (orphaned)
until the user saves or discards them, and closing a workspace with unsaved
changes is refused with a message.

## Data flow

```text
KeyEvent ──► KeyCombo ──► prompt / scope resolution ──► Command
                                                         │
                                                         ▼
                                              Command::apply (dispatch)
                                                         │
                            ┌────────────────────────────┼───────────────────────┐
                            ▼                            ▼                       ▼
                      Document::insert            layout mutation         workspace ops
                            │                            │                       │
                            └──────────────► AppState (rect cache refreshed) ◄───┘
                                                         │
                                                         ▼
                                              ui::compute_layout + ui::render
```

## Testing

- Editor: mutation, coalescing, undo/redo round trip, EOL preservation.
- Text: grapheme motion (combining marks, ZWJ), word motion, cell widths, clipping.
- Layout: proportional splits, min sizes, nesting, collapse, separators.
- Workspaces: switching, closing, last-workspace guard.
- Keymap: parsing, normalization, scope shadowing, bad-override reporting.
- Rendering: `TestBackend` snapshots of the full frame, nested splits, tiny
  terminals, prompt overlays.
- Dispatch: typing, undo, save, cut/paste, split/close, workspace round trip,
  replace-all, focus movement, split rejection at minimum size.
