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
layout change cannot desynchronize hit-testing from drawing. Document tabs inside
a pane reuse the renderer's label widths; clicking one focuses that pane and
activates the document.

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

**Syntax highlighting is structural.** `syntax::tree` owns a tree-sitter parser
per document; Rust, Python, JavaScript/TypeScript, Go, C, Shell, TOML, JSON, and
Markdown load their highlight queries once in a `LazyLock`. `Document` hands every
insert and removal to the tree as an `InputEdit`, so the next frame reparses only
the changed region; the query cursor is bounded to the visible byte range.
Captures map to `TokenKind` by capture-name prefix, and numbers captured as
`constant.builtin` (Rust literals) stay `TokenKind::Number`. Markdown's block tree
is supplemented by the inline grammar for each visible `(inline)` node, the HTML
grammar for `html_block` nodes and inline `html_tag` nodes, the YAML grammar for
`minus_metadata`, the TOML grammar for `plus_metadata`, and the grammar named in
each visible fence's info string; those sub-trees are cached by byte offset and
cleared on every edit.
`SyntaxCache` also keeps the lexer's
per-line state as a fallback when a grammar or query is unavailable; filling
either during rendering is derived work and never changes document content.

**The terminal panel is a real PTY plus a VT100 model.** `shell::Terminals` owns
one `ShellPanel` per tab plus the active index; the panel is open while the
collection is non-empty. Each `ShellPanel` spawns `$SHELL` through `portable-pty`,
reads the master in a dedicated thread into an `mpsc` channel, and drains that
channel in the event loop (`pump_shell`) before each draw. Every shell is pumped
and resized; only the active tab is rendered and receives keys, wheel events, and
mouse reports. `vt100::Parser` turns bytes into a screen grid; rendering maps
cells (colors, bold, underline, inverse) onto the theme and places the terminal
cursor when the panel has focus. Resizing recomputes both the PTY size and the
parser grid, and the panel reserves rows before panes are laid out, so the
minimum pane size still holds. Dropping a `ShellPanel` kills and waits for its
child. The parser also tracks the program's mouse protocol mode and encoding;
while active, mouse events inside the panel body are forwarded as SGR, UTF-8, or
X10 reports gated by that mode, and the wheel no longer scrolls the local
scrollback. Scrollback is clamped to the configured limit, and `encode_key`
emits xterm CSI modifier parameters for navigation/function keys instead of
dropping Ctrl/Alt/Shift.

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
and scroll to `session.toml`. The run loop writes the same snapshot every five
seconds and on exit, so a crash loses at most the last few seconds of layout.
Restore only runs without a CLI path when the saved root matches the current
directory. The snapshot is capped and validated first (version, workspace/pane/
document counts, split child and weight ranges, every pane referenced exactly
once); a failure is reported and the default workspace stays. Only files on disk
are restored, so untitled and scratch buffers never appear as empty documents.

**Self-update follows the release contract.** `--update` queries the latest
release, refuses to move backwards, downloads the platform archive plus its
SHA-256 file through `curl`, verifies with `sha2`, extracts with `tar`, and
installs by copying to a sibling file, setting mode 755, and renaming over the
canonical executable. The running process keeps its mapping until exit, and a
failure leaves the old binary intact. Only the executable changes.

**Theme switching is a runtime state change, not a restart.** `select_theme`,
`next_theme`, and `prev_theme` run through the same command path as everything
else: `Theme::names` merges presets with `themes/*.toml`, `Theme::from_config`
resolves the chosen name, and `AppState::theme` is replaced. Rendering reads the
palette from state each frame, so panes, syntax, explorer, status bar, prompts,
and the terminal panel all switch together.

**Key bindings are data.** `KeyMap` holds three scopes (`global`, `editing`,
`explorer`). Resolution is prompt, focused scope, global. Startup probes
keyboard enhancement support and forces the `safe` preset without it, naming the
dropped actions in the status bar. Conflicts and unknown names are reported with
the config path and line, and a binding cannot take a combination reserved for
quit.

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

**Watching is one recursive notify watcher.** `watch::Watcher` owns the
recommended backend and a channel. The event loop drains that channel once per
frame, drops `Access` events, and coalesces the rest: a visible path rebuilds the
explorer with its selection preserved by path, and each open document whose file
appears in the batch is checked with `disk_changed` — clean buffers reload, dirty
buffers only get a status message. The same batch clears the quick-open cache, so
the project is scanned once instead of once per prompt. Ignored paths still
produce notify events because the backend cannot skip them, but they never
rebuild the tree, which keeps build output from causing churn.

**Find in Files is one worker per query.** `project_search::Task` receives the
cached quick-open list, reads each file, and streams `Message::Matches` batches
over a channel; an atomic flag cancels the previous worker when the query
changes, and a dropped receiver stops it at its next send. The app drains the
channel once per frame, appends labels to the prompt without fuzzy-refiltering
them (they are already matches), and jumps by converting line plus char column
into a document offset. Preview strings are capped to a window around the match
and reuse the current line's char buffer, so a very long line cannot multiply
1000 stored previews. The worker skips files above 1 MiB and invalid UTF-8, and
stops at 1000 matches.

**Replace in Files is the same worker plus one explicit writer.**
`group_by_file` collapses consecutive results per file for the preview list.
Typing never writes; `Enter` on the replacement field is the per-file
confirmation. A clean open buffer is rewritten as a single `Document::replace`
so undo crosses the save, then saved through the atomic path; a dirty buffer is
refused. Any other file is read, rewritten with the same `Matcher`, and replaced
atomically (`write_atomic`, with an in-place fallback when rename is refused).

**Structure comes from the same grammars, parsed on demand.** `structure::symbols`
and `structure::folds` build a fresh tree with the language's tree-sitter grammar
when the user asks for a symbol list or a fold; rendering keeps its separate
incremental tree in `Highlighter`. Symbol rows carry tree-sitter byte columns,
converted to char offsets at the jump. Folds are inclusive line ranges stored on
the `Document`; any edit clears them because their line numbers would drift.
`ui::pane` renders only non-hidden lines and maps display rows back through the
same filter for the cursor, bracket highlights, and mouse hit-testing.

**One pane can hold several cursors.** `Pane.cursor/anchor` is the primary
selection; `Pane.extra` holds secondary carets and selections. `edit_ranges`
merges overlaps into ascending ranges, and `Document::replace_ranges` applies
them from the end while recording every remove/insert in one `History` batch, so
undo and redo cover the whole multi-cursor edit. `cursors_after` computes the
post-edit positions; single-range edits still take the incremental
`insert`/`remove`/`replace` path so tree-sitter updates stay minimal. Mouse
Alt-drag builds one selection per line through `Pane::column_selection`. Every
caret is rendered: the terminal owns one hardware cursor for the primary caret,
while `ui::pane` paints each secondary caret with reverse video at the same
visual row/column mapping used for clicks and wrapped movement.

**Soft wrap is a pure line-to-row mapping.** `text::wrap_ranges` splits a line by
display cells with a word-boundary preference; `text::visual_rows` maps document
lines to `VisualRow { line, start, end }`, skipping folds. Rendering, cursor
placement, mouse hit-testing, and vertical movement all consume that same list,
so wrapped rows cannot disagree between surfaces. Vertical movement does not
build the list for the whole document: `wrapped_vertical` wraps the cursor's line
and walks only the adjacent visible lines, one viewport step at a time. Wrapping
is enabled per document set (`editor.word_wrap`) and horizontal scrolling turns
off while it is active.

**Line endings are converted as a document edit.** `Document::convert_eol`
normalizes the rope to the requested ending, records the whole replacement in
one undo batch, and updates `Eol` so new lines and the next save use it. Normal
saves still write the rope verbatim, so untouched mixed endings survive.

**Lua plugins share one sandboxed state and a registry.** `plugins::Plugins`
loads the plugin directory into a `Lua` created with only the safe string/table/
math/utf8/coroutine libraries; `io`/`os` are added only by capability, and
`package` is never loaded. Each plugin gets API tables whose closures record
command ids, key combos, event hooks, and statusline hooks in a shared
`Registry`. Callbacks are Lua functions stored in a hidden table; invoking one
sets a per-call `__i3_ctx` snapshot (`path`, language, cursor, selections) that
the read-only `editor` table reads. A per-call instruction hook aborts past the
time budget, a depth counter stops re-entrancy, and repeated errors disable a
plugin. Reloading replaces both the Lua state and the registry, which unregisters
everything atomically. The watcher watches the plugin directory next to
`config.toml` and routes those events to `App::reload_plugins`.

**Git is a thin CLI adapter.** `git::line_marks` parses `git diff HEAD -U0` hunk
headers into one marker per new-side line; untracked files are read and marked
added. `git::status` parses porcelain `XY` codes, `diff` returns unified text for
the read-only overlay, and stage/unstage/commit call `git add`, `git restore
--staged`, and `git commit`. `git::hunks` splits a single-file diff at `@@`
headers and reuses the file header for each patch; `git_stage_hunk` feeds one
patch to `git apply --cached`, and `git_unstage_hunk` adds `--reverse`. The hunk
picker is a selectable overlay over that list. `git::branch`, `git::branches`,
and `git::log` feed the read-only branch/log overlay; they handle detached HEAD
and the unborn-repository case. `git::pull` refuses uncommitted changes and uses
`--ff-only`; `git::push` uses the configured upstream. Both run with terminal
prompts disabled. `conflict::blocks` finds complete `<<<<<<<`/`=======`/`>>>>>>>`
groups; `App::resolve_conflict` replaces one block with ours, theirs, or both as
a single document edit, and `git_conflicts` opens a filtered status prompt.
`App::refresh_git_marks`
recomputes only documents shown in panes, once per second and after
open/save/git actions. Every helper returns git's stderr/stdout on failure, which
the command path puts in the status bar.

**LSP is threads and channels, not a runtime.** `lsp::LspManager` owns one
`Server` per configured language. Spawning pipes a child's stdin/stdout; a reader
thread frames Content-Length messages into an mpsc channel; the UI thread drains
that channel in `App::pump_lsp` once per frame. Requests carry an id whose
`ReplyKind` lives in the server's pending map, so completion, hover, and
definition replies need no global dispatcher. `initialize` runs first; requests
sent before the response are queued per server and flushed once it arrives.
Documents use full sync: every frame compares the rope text with the last sent
text and emits `didOpen`/`didChange`/`didClose`; positions convert between char
offsets and UTF-16 columns at the boundary. Diagnostics are stored per document
id and rendered by the gutter and status bar. A server that fails to spawn is
marked failed and reported once, so edits never wait on a language server.
Pending requests carry a send timestamp: an unanswered `initialize` fails the
server after 60 s, and other requests are dropped after 15 s. Changing the
workspace root or the `[lsp.*]` tables calls `LspManager::restart`, which drops
the old children and reopens the documents against the new root/config on the
next pump.

**File operations follow the focused surface.** `duplicate_file`, `copy_path`,
and `reveal_in_terminal` take the explorer row when the explorer has focus;
otherwise the active document's path wins, with the explorer row as fallback.
Duplication writes a sibling `name copy` file inside the workspace root and
refreshes the tree, copying a path never renames anything, and reveal starts a
shell in the target directory without changing the workspace root. Workspace
names persist in `session.toml`; terminal titles live on `ShellPanel` and reset
with the shell.

**Search history is per surface and lives outside config.** `SearchHistory`
stores the last 50 submitted queries for Find, Find in Files, Replace, and
Replace in Files plus the shared search options in `history.toml`. Prompt state
copies the list for its surface; a successful submit pushes the query, and
`Alt+Up`/`Alt+Down` walks it. The list is boxed in `PromptState` so `InputMode`
stays small.

**Help is derived from the resolved keymap.** `help` walks the global, editing,
and explorer binding maps and the palette command list at open time, so it shows
`config.toml` overrides and the safe preset without a second source of truth.
The command palette uses the same maps through `KeyMap::active_binding`, drawing
the focused-scope binding (or the global fallback) at the right edge of each row.

**Scratch buffers are ordinary documents without a path.** `new_scratch` creates
`Document::empty(id, None)`, so editing, undo, Save As, and lua hooks all use the
existing paths; session snapshots and crash recovery skip documents without a
path. Session autosave keeps a path and a five-second clock on `App` and reuses
`session::save_to` for both autosave and exit.

**Terminal directories are separate from workspace roots.** `Workspace` stores
`terminal_dir`, initialized to the root, changed by `workspace_terminal_dir`, and
persisted in `session.toml`. A root change follows into `terminal_dir` only when
the terminal directory still pointed at the old root.

**Each workspace owns a root and a watcher.** `Workspace.root` is persisted in
`session.toml`. `AppState` keeps one explorer for the active workspace; switching
rebuilds it from the target root and clears the quick-open cache. `App.watchers`
holds one recursive watcher per workspace id; the loop drains all of them,
coalesces paths, and only rebuilds the explorer when the event came from the
active workspace. Open buffers react to their own paths regardless of workspace.
A second non-recursive watcher covers the parent directories of open documents
outside every workspace root; `sync_external_watcher` adds and removes those
directories as documents open and close, and drops the watcher when none remain.

**Crash recovery is a swap directory, not a journal.** `recovery::write_to`
writes each dirty document to `<config>/i3-editor/swap/` through `tempfile` +
rename, keyed by the hex-encoded path, and deletes swaps for clean documents.
The event loop runs it at most once per second. Startup replays swaps into
`Document::restore` (fresh undo history, dirty flag), attaches recovered buffers
as tabs, and prompts per file: save, reload, or keep. A graceful exit clears the
directory, so discarded quit choices cannot come back.

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
changes is refused with a message. Quit always opens a confirmation prompt:
clean sessions offer Quit/Cancel, dirty sessions add Save all/Discard all.

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
