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

**Theme colors come from presets or files, resolved once at startup.**
`appearance.theme` first looks for `<config>/i3-editor/themes/<name>.toml`, then a
built-in preset, then falls back to `default` with the available names in the
message. A theme file may declare `base` to inherit a preset, and every field is
optional; unknown keys are rejected by `serde(deny_unknown_fields)` so a typo is
reported rather than ignored. Colors are parsed from `#rgb`, `#rrggbb`,
`indexed:N`, or names. The resolved palette lives in `AppState::theme`, so
rendering stays read-only. Text styles always set both foreground and background:
Ratatui writes the style it is given per cell, so a `Style::default()` span would
erase a themed background.

**Syntax highlighting is lexical but stateful.** `syntax::line_spans` consumes the
lexer state at the start of a line and returns the state for the next one, so
block comments, triple-quoted strings, template literals, raw strings, and
Markdown fences keep their token kind across line breaks. `Document` memoizes one
start state per line in `SyntaxCache`, invalidates from the edited line, and
re-lexes only the lines a frame asks for; filling that memo during rendering is
derived work and never changes document content. No grammar dependency is
involved, and the cost stays one pass over the visible lines plus the gap between
the last cached line and the viewport. Identifiers are still classified by
convention; structural highlighting remains the Tree-sitter upgrade path.

**The terminal panel is a real PTY plus a VT100 model.** `shell::ShellPanel`
spawns `$SHELL` through `portable-pty`, reads the master in a dedicated thread into
an `mpsc` channel, and drains that channel in the event loop (`pump_shell`) before
each draw. `vt100::Parser` turns bytes into a screen grid; rendering maps cells
(colors, bold, underline, inverse) onto the theme and places the terminal cursor
when the panel has focus. Resizing recomputes both the PTY size and the parser
grid, and the panel reserves rows before panes are laid out, so the minimum pane
size still holds. Dropping `ShellPanel` kills the child. Mouse reporting is not
forwarded into the shell — clicks only focus the panel, and the wheel scrolls the
model's scrollback.

**Nothing modified is discarded without an explicit choice.** `Ctrl+Q` with
modified documents opens a prompt offering save-all/discard-all (discard only for
untitled buffers, which cannot be saved blindly); `F6` refuses to close a pane
whose tabs include modified documents, so no document becomes unreachable. Both
paths reuse the existing command/prompt machinery, so the guard is testable
without a terminal.

**Disk state is tracked, not assumed.** A document records the mtime and size it
saw at load or save. If the file changed underneath a modified buffer, `Ctrl+S`
opens a prompt offering `Overwrite with the buffer` or `Reload from disk`; the
write is deferred until the choice. Reload replaces the rope, drops undo history,
clears the syntax memo, and clamps cursors in every pane showing that document.

**Sessions are validated snapshots.** `src/session.rs` serializes workspaces,
layout trees (axis, tiling/tabbed mode, active child, weights), pane tabs, cursor,
and scroll to `session.toml`. Restore only runs without a CLI path when the saved
root matches the current directory. The snapshot is capped and validated first
(version, workspace/pane/document counts, split child and weight ranges, every
pane referenced exactly once); a failure is reported and the default workspace
stays. Only files on disk are restored, so untitled buffers never appear as empty
documents.

**Theme switching is a runtime state change, not a restart.** `select_theme`,
`next_theme`, and `prev_theme` run through the same command path as everything
else: `Theme::names` merges presets with `themes/*.toml`, `Theme::from_config`
resolves the chosen name, and `AppState::theme` is replaced. Rendering reads the
palette from state each frame, so panes, syntax, explorer, status bar, prompts,
and the terminal panel all switch together.

**Key bindings are data.** `KeyMap` holds three scopes (`global`, `editing`,
`explorer`). Resolution is prompt, focused scope, global. Startup probes
keyboard enhancement support, falls back to the `safe` preset, and reports
conflicts instead of silently dropping a binding.

**Undo is an operation log with revisions.** Ropey has no history. Each `Edit`
records the char index and the text it inserted or removed; each `Batch` carries
cursor/scroll restoration plus `id`/`parent` revisions. `History` tracks `current`
and `saved`, so `Document::dirty` is derived from the revision position rather than
set blindly: undo back to the saved state clears the marker, redo restores it.
Depth is capped at 1000 batches, dropping the oldest so long sessions stay bounded.

**Cursor positions are char indices.** Byte offsets and display columns are
derived at the boundary by `editor::text`, which uses `unicode-segmentation` for
graphemes and `unicode-width` for cells.

**Prompts are one subsystem.** Quick open, find, replace, save-as, new file,
rename, delete, discard, and go-to-line all use `PromptState`. Candidate prompts
are ranked by Nucleo (`Pattern::parse` + `match_list`, smart case, path-boundary
bonuses); `keep_selection` preserves an arrow-key choice while the query narrows.
While a prompt is active it consumes every key except the quit binding, so a
dialog cannot leak input into a document.

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
