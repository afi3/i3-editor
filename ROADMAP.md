# Roadmap

Status of the phases defined in `MASTER.md`. Each phase was implemented and
verified before the next one started.

## Phase 1 — Terminal foundation and pane skeleton: done

- Raw mode, alternate screen, bracketed paste, mouse capture, keyboard
  enhancement probe with fallback, panic hook and signal restore.
- Event loop with `AppState`, one pane, one workspace, explorer/editor/status
  regions, resize handling.

Checked by: `renders_status_bar_and_document`, `stays_inside_a_tiny_terminal`,
plus an interactive pty run (draw at 80×24, `Ctrl+Q` restores the terminal,
exit code 0).

## Phase 2 — Keymap, input routing, prompts: done

- `KeyCombo` parsing/normalization, TOML overrides, conflict reporting, `safe`
  preset, `--dump-keymap`.
- `InputMode` routing; one `PromptState` for palette, quick open, find, replace,
  save-as, new file/dir, rename, delete, discard, go-to-line.

Checked by: keymap tests, preset tests, dispatch tests for replace-all.

## Phase 3 — File explorer: done

Lazy per-directory loading, `.gitignore` filtering via `ignore`,
expand/collapse, hidden toggle, create/rename/delete with confirmation prompts,
plus mouse selection, double-click open, and wheel navigation.

## Phase 4 — Editor engine: done

Ropey document, grapheme cursor, selection, insertion/deletion, auto-indent,
scrolling with saturation, line numbers, EOL-preserving save, dirty tracking,
undo/redo with coalescing, plus click-to-cursor, shift-click selection, and wheel
scrolling.

## Phase 5 — Document tabs: done

Per-pane tab strip, document registry, cycle bindings, dirty-close prompt.

## Phase 6 — Split layout engine: done

Weighted multi-child splits, nesting, minimum-size enforcement, separator drag
resize, pane focus by geometry, close with tab handling, collapse invariants,
a fullscreen toggle that hides siblings without changing the tree, pane moving by
swapping leaf ids in grab mode (`F10` + arrows), and i3-style tabbed containers
(`F12`) with keyboard/mouse tab switching, per-container strips, and tab creation
by splitting inside a container.

## Phase 7 — Workspaces: done

Create/switch/close, workspace tab bar, preserved layout, tabs, cursor, scroll.

## Phase 8 — MVP stabilization: done (v0.1.2)

Delivered:

- Config loading with per-section fallback and warnings; unknown sections,
  unknown keys, and invalid values are reported with the config path and line,
  and binding warnings name the failing entry.
- Startup capability handling: without keyboard enhancement the safe preset is
  forced and one status message names the actions it removes; `next_workspace`
  and `prev_workspace` keep workspaces reachable from the palette, user bindings
  cannot shadow quit, and `Ctrl+C` cancels an active prompt.
- `I3_EDITOR_DEBUG=1` writes the resolved keymap to `<config>/i3-editor/keymap.log`
  at startup.
- Bracketed paste is routed to the prompt field, shell PTY, or document by owner,
  and an open prompt captures mouse input instead of editing behind it.
- The status bar shows the first warning plus the remaining count, and the
  `messages` palette command lists every warning.
- Palette coverage: every explorer action and each open workspace
  (`switch_workspace_N`) is reachable from `F1`.
- `Ctrl+O` opens a typed path and `Ctrl+N` creates a file on disk instead of an
  untitled buffer; `--dump-keymap` probes the real terminal before resolving.
- OSC 52 payloads above 64 KiB are skipped after the internal clipboard is set,
  with a status-bar explanation.
- Theme presets (`default`, `dark`, `light`, `high-contrast`) and theme files
  loaded by name from `<config>/i3-editor/themes/`, with `base` inheritance.
- Tree-sitter highlighting for Rust, Python, JavaScript/TypeScript, Go, C,
  Shell, TOML, JSON, and Markdown: each document keeps its parse tree, every edit
  becomes an `InputEdit` for an incremental reparse, and the highlight query runs
  over the visible byte range. Markdown adds the inline grammar per visible
  inline node, the HTML grammar for HTML blocks and inline tags, the YAML grammar
  for `---` front matter, the TOML grammar for `+++` front matter, and parses
  each fenced block with the language from its info string; those sub-trees are
  cached and cleared on edit.
- Bottom terminal panel running real shells (`portable-pty` + `vt100`) in tabs,
  opened and closed from the palette (`F5` new, `F11` close), with theme-aware
  rendering, per-tab scrollback, and `Ctrl+PageUp/PageDown` cycling.
- Terminal mouse reporting: when the shell program enables `DECSET 1000/1002/1003`,
  clicks, drags, and wheel events in the panel body are forwarded with the
  requested encoding (SGR, UTF-8, or X10), gated by its protocol mode.
- Runtime theme switching from the palette (`select_theme`, `next_theme`,
  `prev_theme`), persisted back into `config.toml` without disturbing other keys.
- Live find: the prompt lists matching lines with line numbers, `Up`/`Down`
  select a match and wrap, every match is highlighted in the buffer, and
  `next_match` advances from the selected one instead of re-selecting it.
- Fuzzy finder for quick open and the command palette, scored by Nucleo with
  smart case and path-boundary bonuses; arrow selections survive a narrowing
  query and otherwise the best match stays selected.
- Find in Files: a background worker scans the quick-open file list with literal
  smart-case matching, streams `path:line:column` results into a prompt, cancels
  older scans on each keystroke, and jumps to the selected match. Files above
  1 MiB, non-UTF-8 files, and matches past 1000 are skipped and reported.
- Stateful lexer retained as a fallback when a grammar or query is unavailable:
  cross-line block comments, triple-quoted strings, template literals, and
  Markdown fences. Per-line lexer states are memoized per document.
- Saves replace the file atomically (temp file + rename) with permissions and
  symlinks preserved; Save As confirms before replacing another file. A save after
  an external change asks to overwrite or reload instead of clobbering it.
- File watching with `notify`: one recursive watcher on the workspace root feeds
  the event loop. Create/rename/delete refreshes the explorer while preserving
  the selected path, a clean buffer reloads when another program writes it, a
  modified buffer only reports the change, and the quick-open list is scanned
  once and invalidated by the same events.
- Session persistence: workspaces, layouts, pane tabs, cursor, and scroll are
  written to `session.toml` and restored when launched without a path in the same
  directory; untitled buffers are skipped and broken sessions are ignored.
- Terminal panel honors `[terminal] shell` and `[terminal] scrollback`.
- Self-update: `i3-editor --update` checks the latest GitHub release, refuses
  downgrades, verifies the SHA-256, and atomically replaces the binary; README
  documents the tag-based release path.
- `Tab` honors `editor.insert_spaces`: spaces to the next tab stop or a literal
  tab; `explorer.visible` is applied at startup.
- Quit confirmation with save-all/discard-all, pane-close guard for modified
  documents, and bounded undo (1000 batches) that tracks the saved revision.
- Keymap presets and capability handling.
- Unicode width/grapheme handling with tests.
- Error handling through a single `Error` type; status-bar messages.
- 231 unit and integration tests, `TestBackend` render tests.
- `install.sh` one-liner with `--update` and `--uninstall` (checksum-verified
  release download, version comparison, Cargo fallback) and a tag-driven release
  workflow for macOS/Linux on x86_64 and aarch64.
- README demo GIF recorded from the real binary (`scripts/make_demo.py`).
- `README.md`, `ARCHITECTURE.md`, `ROADMAP.md`.

Known MVP limit (not a blocker):

- An unknown fence language stays plain text, as documented in `README.md`.

## Release v0.1.3 — shipped

Shipped as `v0.1.3` on 2026-11-22. Every feature previously spread across
v0.1.3–v0.1.9 shipped in this single release; there are no intermediate tags.

Release record:

- The six follow-up workstreams share `state.rs`, `command.rs`, `app/mod.rs`, and
  the prompt/UI files, so they were committed together with the earlier
  search/navigation work as one green feature commit. The version bump and this
  release record are a separate commit.
- `cargo fmt --check`, `cargo check`, `cargo test` (308), and
  `cargo clippy --all-targets --all-features` are clean; `forgeguard gate
  --changed` reports 0 errors and 0 warnings.
- Version `0.1.3`, tag `v0.1.3`, and the four platform assets are the release
  milestone; `main` remains the development branch.

Policy that applied during the cycle:

- `Cargo.toml` stayed at `0.1.2` until every workstream below passed acceptance.
- The version was bumped only after the overall checklist below was satisfied.
- Keep this section current: the progress list and test count are updated as work
  lands, so a new session does not rescan the codebase.

Delivery order (dependencies only, not separate releases):

1. Editing core + Lua language support
2. Search and replace parity
3. Navigation, workspace roots, recovery
4. LSP MVP
5. Git integration
6. Editing model expansion
7. Lua extension host

Internal ordering rules:

- Lua **language support** lands in workstream 1; the extension host is last.
- The Lua host waits for LSP and per-workspace roots because both reshape the
  APIs it exposes.
- The Lua API exposes selections as a **list** from v1 even when the editor holds
  one cursor, so multi-cursor never breaks the plugin contract.
- Git may move ahead of LSP only by an explicit decision; LSP remains the
  architectural gate.
- Every workstream ends with its own tests, `cargo fmt --check`,
  `cargo clippy --all-targets --all-features`, and a clean forgeguard gate.

Risk: this is a large single release, so a regression has a wider bisect range.
Mitigation: one commit per workstream, no feature is marked done without its own
acceptance, and the version is bumped only after the full checklist passes.

Progress for v0.1.3:

- [x] 1. Editing core + Lua language support
- [x] 2. Search and replace parity
- [x] 3. Navigation, workspace roots, recovery
- [x] 4. LSP MVP
- [x] 5. Git integration
- [x] 6. Editing model expansion
- [x] 7. Lua extension host

### 1. Editing core + Lua language support

Status: implemented and committed; release waits for the rest of v0.1.3.

- Lua language support: `tree-sitter-lua`, `Language::Lua`, `.lua`, highlight
  query, and `lua` as a Markdown fence language.
- Line operations: move line/selection up and down, delete line.
- Toggle line and block comments per document language.
- Auto-pair brackets and quotes; skip over an existing closer on typing.
- Bracket matching highlight under the cursor.
- Mouse drag selection inside a pane.
- Wheel scrolling in prompts, including Find in Files and Messages.
- Clicking a workspace tab switches workspaces.
- Config hot reload: watch `config.toml`, then re-resolve keymap and theme.

Touches: `src/syntax/mod.rs`, `src/syntax/tree.rs`, `src/command.rs`,
`src/app/editing.rs`, `src/app/lines.rs`, `src/app/comment.rs`,
`src/app/cursor.rs`, `src/app/mouse.rs`, `src/app/config_ops.rs`,
`src/app/watch_ops.rs`, `src/input/presets.rs`, `src/editor/document.rs`,
`src/editor/history.rs`, `src/ui/pane.rs`, `src/ui/mod.rs`,
`src/config/mod.rs`, `src/watch.rs`, `Cargo.toml`.

Acceptance: Lua fixture and `lua` fence highlight; line move/delete and comment
toggle are single undo steps; auto-pair/skip and bracket emphasis tested;
drag-select, prompt wheel, workspace click, and config hot reload tested at
80×24; full suite and gate clean.

### 2. Search and replace parity

Status: implemented and verified; commit and release wait for the rest of
v0.1.3.

- `search_options::Matcher` is one compiled pattern shared by Find, Replace,
  Find in Files, and Replace in Files: literal (escaped, `$` stays literal) or
  regex, whole word, and case sensitivity; char offsets throughout.
- `Alt+R` / `Alt+W` / `Alt+C` toggle the options inside any search prompt; the
  prompt title shows the flags and refreshes the preview.
- Find and Replace report `index/total` and the total with a `+` when the
  10 000-match cap truncated the list; Find in Files and Replace in Files report
  the streamed match count and `+` on truncation.
- Replace in Files previews grouped results (`relative (N matches)`) without
  writing. `Enter` moves from query to replacement; `Enter` on the replacement
  (or `Ctrl+Enter`) writes the selected file only, keeps the prompt open, and
  rescans. Clean open buffers are replaced as one undo step and saved through
  the atomic path; dirty buffers are refused. Other files use the same
  `Matcher` and atomic write.

Touches: `src/search_options.rs`, `src/app/search.rs`, `src/project_search.rs`,
`src/app/project_search.rs`, `src/input/prompt.rs`, `src/ui/overlay.rs`,
`src/command.rs`, `src/editor/document.rs`.

Acceptance: regex and literal tests including invalid patterns, whole-word at
boundaries, replace preview does not write before confirmation, and a replace
across two files is verified by reading them back.

### 3. Navigation, workspace roots, recovery

Status: implemented and verified; commit and release wait for the rest of
v0.1.3.

- `structure::symbols` parses the document with its tree-sitter grammar on
  demand and lists Rust, Python, JavaScript/TypeScript, Go, C, Shell, Lua,
  Markdown heading, and TOML table definitions in source order. `go_to_symbol`
  filters them with the normal prompt ranking and jumps to the line and char
  column, opening a fold that hid it.
- `structure::folds` returns inclusive line ranges for declarations, blocks,
  match expressions, Markdown sections/fences, and TOML tables. `toggle_fold`
  stores the smallest range starting at the cursor line on the document,
  unfolds the covering range on the next call, and any edit clears the list.
  `ui::pane` renders only visible lines; cursor placement, bracket emphasis, and
  mouse hit-testing use the same filter.
- `Workspace` owns a `root` that is persisted in `session.toml`. Switching
  workspaces rebuilds `AppState.explorer` for that root and clears the
  quick-open cache. `workspace_root` (palette) changes the active root and
  replaces its watcher; a new workspace inherits the current root. `App` keeps
  one recursive watcher per workspace; inactive workspace events only reload
  open buffers, never rebuild the active tree.
- Crash recovery writes dirty buffers to `<config>/i3-editor/swap/` at most
  once per second through a `tempfile` + rename, removes swaps for clean files,
  and clears the directory on a graceful exit. Startup replays each swap into
  `Document::restore`, attaches it as a tab, and prompts per file: save,
  reload from disk, or keep unsaved (`Esc` keeps).

Touches: `src/structure.rs`, `src/app/outline.rs`, `src/app/folds.rs`,
`src/app/recovery_ops.rs`, `src/recovery.rs`, `src/editor/document.rs`,
`src/state.rs`, `src/input/prompt.rs`, `src/app/prompt.rs`, `src/command.rs`,
`src/workspace/mod.rs`, `src/app/workspace_ops.rs`, `src/app/watch_ops.rs`,
`src/app/mod.rs`, `src/session.rs`, `src/ui/pane.rs`, `src/ui/overlay.rs`,
`src/main.rs`.

Acceptance: outline jump lands on the symbol line, folding hides the expected
range, switching workspaces changes the explorer root and quick-open list,
crash recovery restores a dirty buffer after a synthetic exit without save.

### 4. LSP MVP

Status: implemented and verified; commit and release wait for the rest of
v0.1.3.

- `lsp::LspManager` spawns one stdio server per configured language lazily when
  a matching document opens. A reader thread frames Content-Length JSON-RPC
  messages into a channel; the UI thread pumps them once per frame, so no
  runtime and no blocking reads. A server that fails to spawn is reported once
  and marked failed; edits never wait for it.
- Lifecycle: `initialize` with root URI and text-document capabilities, then
  `initialized`; requests sent before that are queued per server and flushed on
  the response. Documents use full-text `didOpen`/`didChange`/`didClose` with a
  version counter; positions convert between char offsets and UTF-16 columns.
- Diagnostics from `textDocument/publishDiagnostics` are stored per document and
  rendered as severity-colored `•` gutter markers plus `LSP nE mW` in the status
  bar.
- `lsp_completion` opens a popup with `Up`/`Down`/`Enter`/`Esc`; `Enter` inserts
  the item's `textEdit.newText`/`insertText`/label. `lsp_hover` shows server text
  in a bordered popup. `lsp_definition` accepts `Location` and `LocationLink`
  replies, opens the file, unfolds, and places the cursor.
- Config adds `[lsp.<name>]` tables with `command`, `args`, and optional
  `extensions`; an empty extension list matches the table name's language.

Touches: new `src/lsp/mod.rs`, new `src/app/lsp_ops.rs`, `src/app/mod.rs`,
`src/app/dispatch.rs`, `src/command.rs`, `src/config/mod.rs`, `src/state.rs`,
`src/ui/mod.rs`, `src/ui/overlay.rs`, `src/ui/pane.rs`, `src/main.rs`.

Acceptance: a fake or real server fixture produces a diagnostic, completion
popup, hover text, and a definition jump in tests or a scripted pty check.

Risk: largest architectural change; write the client/lifecycle design before
implementation. Keep completion optional so a missing server never blocks edits.

### 5. Git integration

Status: implemented and verified; commit and release wait for the rest of
v0.1.3.

- `git::line_marks` compares the worktree against `HEAD` with `git diff -U0` and
  maps hunks to per-line markers: added, modified, and deleted. Untracked files
  are read and marked as entirely added. Markers refresh after open/save, after
  Git commands, and at most once per second for documents shown in panes.
- The gutter has a Git column (`+`, `~`, `-`) next to the LSP column. The status
  bar keeps its LSP counts.
- `git_status` lists porcelain `XY path` entries in a prompt; `Enter` opens the
  file's unified diff. `git_diff` opens the active file's diff directly. Both
  use a read-only overlay (scroll keys, `Esc`/`q` closes) and never write.
- `git_stage` / `git_unstage` act on the active file; `git_commit` prompts for a
  message and commits the staged index. Errors from every helper (missing repo,
  nothing staged, git absent, bad path) reach the status bar.
- `[git] binary` selects the executable; the default is `git`. No libgit2.

Touches: new `src/git.rs`, new `src/app/git_ops.rs`, `src/app/mod.rs`,
`src/app/dispatch.rs`, `src/app/prompt.rs`, `src/command.rs`,
`src/config/mod.rs`, `src/state.rs`, `src/ui/mod.rs`, `src/ui/overlay.rs`,
`src/ui/pane.rs`, `src/error.rs`, `src/main.rs`.

Acceptance: fixture repository shows correct markers, diff view opens without
modifying files, staging changes the index, and every command reports git errors
in the status bar.

### 6. Editing model expansion

Status: implemented and verified; commit and release wait for the rest of
v0.1.3.

- `Pane.extra` holds secondary carets/selections. `add_cursor_below` /
  `add_cursor_above` add them at the same column; `clear_cursors` removes them.
  Alt-drag builds a rectangular column selection through
  `Pane::column_selection`. Typing, newline, tab, backspace/delete, paste, copy,
  and cut operate on every range; `Document::replace_ranges` records one
  history batch so undo/redo cover the whole edit. Single-range edits keep the
  incremental tree-sitter path.
- Soft wrap: `[editor] word_wrap` or `toggle_word_wrap` (palette).
  `text::wrap_ranges` splits lines by display cells preferring spaces;
  `text::visual_rows` maps lines to rows skipping folds. Rendering, cursor
  placement, Up/Down movement, and mouse hit-testing share that mapping.
  Horizontal scroll is off while wrapping.
- Line endings: `line_endings_lf` / `line_endings_crlf` convert the document in
  one undo step, update `Eol` (status bar and new-line insertion), and the next
  save writes the converted bytes. Normal saves still preserve untouched mixed
  endings. Non-UTF-8 files remain rejected with `not valid UTF-8`.

Touches: `src/state.rs`, `src/app/cursor.rs`, `src/app/editing.rs`,
`src/app/mouse.rs`, `src/app/dispatch.rs`, `src/command.rs`,
`src/editor/document.rs`, `src/editor/history.rs`, `src/editor/text.rs`,
`src/config/mod.rs`, `src/ui/pane.rs`.

Acceptance: multi-cursor edit/undo tests, wrapped-cursor movement and click
tests, CRLF conversion round-trip, and non-UTF-8 rejection with a clear message.

Risk: multi-cursor and wrap change invariants used by search, copy, mouse, and
undo; land them behind small, tested steps rather than one change.

### 7. Lua extension host

Status: implemented and verified; commit and release wait for the rest of
v0.1.3.

- `plugins::Plugins` embeds `mlua` 0.12 (vendored Lua 5.4) and loads
  `<config>/i3-editor/plugins/*.lua` in sorted order. Reload replaces the Lua
  state and registry, unregistering every command, keymap, hook, and statusline
  segment first.
- API v1 (`api_version = 1`): `commands.register(name, fn)`, `keymap.set(binding,
  command)`, `events.on(open|save|change|focus, fn)`,
  `statusline.register(fn)`, and read-only `editor.path()`,
  `editor.language()`, `editor.cursor()` (0-based line/column),
  `editor.selections()` (always a list of `{start, end}`). Plugin commands are
  offered in the command palette; key bindings bypass the TOML keymap.
- Callbacks run on the UI thread with an every-10k-instruction hook and a
  configurable deadline (10–5000 ms). Exceeding it aborts and reports. Depth is
  capped at four nested callbacks. Statusline segments are recomputed once per
  input event, never from `draw`.
- Sandbox: only string/table/math/utf8/coroutine are loaded. `allow_file` adds
  `io` and `fs.write_file`; `allow_process` adds `os` and `process.run`; `package`
  is never available. Errors include the plugin file/line, are pushed to
  `messages`, and three failures disable the plugin.
- Watcher covers the plugin directory next to `config.toml`; those events reload
  plugins without touching the explorer. TOML remains authoritative and
  `init.lua` is not supported.
- Demo plugins live in `plugins/` (`hello.lua`, `statusline.lua`,
  `save-marker.lua`).

Touches: new `src/plugins.rs`, new `src/app/plugin_ops.rs`, `src/app/mod.rs`,
`src/app/prompt.rs`, `src/app/editing.rs`, `src/app/search.rs`,
`src/app/workspace_ops.rs`, `src/app/watch_ops.rs`, `src/watch.rs`,
`src/config/mod.rs`, `src/state.rs`, `src/ui/mod.rs`, `src/main.rs`,
`Cargo.toml`.

Acceptance: a fixture script registers a command and keymap, a save hook writes a
marker, a statusline segment renders, `editor.selections()` returns a list, a
broken script reports `path:line` without crashing, reload removes old handlers,
and a budget overflow is reported and stopped.

Risk: API churn and sandbox escapes. Version the API (`api_version`) from the
first release, keep host capabilities narrow, and do not call Lua per frame.

### Overall acceptance for v0.1.3

Shipped: every workstream acceptance passes; `cargo fmt --check`,
`cargo check`, `cargo test` (308), and `cargo clippy --all-targets
--all-features` are clean; `forgeguard gate --changed` reports no errors or
warnings; README, ARCHITECTURE, and this roadmap reflect the shipped features.
Version `0.1.3` is tagged `v0.1.3`, and the release workflow publishes the four
platform archives with their checksums.

### Unscoped backlog

Recorded so the next planning session does not rescan. These are **not** part of
v0.1.3 unless explicitly promoted into a workstream above.

- File operations: duplicate, copy path, reveal in terminal.
- Terminal polish: rename tabs, per-workspace starting directory.
- Workspace polish: rename workspaces.
- Help/keybinding overlay and palette hints showing the active binding.
- Session autosave and scratch buffers (if crash recovery in 0.1.5 is not enough).
- Search history and per-surface option persistence.

## Verification commands

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features
cargo run -- --dump-keymap
```

Last recorded run: `cargo test` → 308 passed, `cargo clippy` → 0 warnings.
