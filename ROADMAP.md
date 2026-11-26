# Roadmap

Status of the phases defined in `MASTER.md`. Each phase was implemented and
verified before the next one started.

## Version policy

- `v0.1.x` is the current hardening and feature-completion line: release
  engineering, correctness, daily-driver operations, performance, polish, and
  Git depth.
- `v0.2.x` covers MCP and AI features. `v0.2.0` shipped the stdio MCP client;
  `v0.2.1` shipped the bounded agent harness, native tools, HTTP MCP, resources,
  prompts, sampling, and approval-gated background agents. `v0.2.2` completes
  the macOS/Linux harness with automatic context, hard budgets, non-Rust LSP
  verification, MCP subscriptions, and Windows source/keyring support.
  Do not tag `v0.3.0` until the remaining `v0.2.x` work lands.

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

## Post-v0.1.3 — correctness pass: done

Landed on 2026-11-22; released in v0.1.4.

- Secondary carets are rendered with reverse video using the same visual-row
  mapping as the primary cursor; secondary selections keep the normal selection
  color.
- LSP servers restart when the workspace root or any `[lsp.*]` table changes.
  Unanswered `initialize` requests time out after 60 s and fail that server;
  completion, hover, and definition requests time out after 15 s.
- README, ARCHITECTURE, and this roadmap no longer claim that only the primary
  caret is drawn or that the LSP client has no timeout.

Verification: `cargo test` (314), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

## Post-v0.1.3 — daily-driver operations: done

Landed on 2026-11-22; released in v0.1.4.

- `duplicate_file` copies the focused explorer file or active document to a free
  `name copy` name; dirty buffers are refused.
- `copy_path` writes the absolute target path to the internal and OSC 52
  clipboards. `reveal_in_terminal` opens a shell tab at the target directory.
- `rename_workspace` renames the active workspace tab.
- `rename_terminal` renames the active terminal tab for the current run.
- Every command is in the palette and covered by app-level tests.

Verification: `cargo test` (320), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

## Post-v0.1.3 — performance pass: done

Landed on 2026-11-22; released in v0.1.4.

- Wrapped `Up`/`Down`/`PageUp`/`PageDown` no longer build every visual row in the
  document. `wrapped_vertical` wraps the cursor's line and walks only the
  next/previous visible line, so the work is bounded by the viewport.
- Release benchmark (5 000 lines, 50 words each, 200 `Down` presses):
  **1.18 s → 2.9 ms** (about 400×).
- Open documents outside every workspace root now get a non-recursive parent
  directory watcher, added and removed as documents open and close; the last
  external document drops the watcher.
- Tests cover cross-line/folded wrapped movement and external-file reload plus
  watcher cleanup.

Verification: `cargo test` (322), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

## Post-v0.1.3 — polish pass: done

Landed on 2026-11-22; released in v0.1.4.

- The `help` command opens a scrollable overlay built from the active global,
  editing, and explorer keymaps plus the palette-only commands.
- Search history stores the last 50 queries per surface and the shared search
  options in `history.toml`; `Alt+Up` / `Alt+Down` cycle the list.
- `session.toml` autosaves every five seconds and on exit. Scratch buffers
  (`new_scratch`) are ordinary pathless documents saved through Save As.
- `workspace_terminal_dir` sets the starting directory for new terminal tabs
  per workspace; the value is stored in `session.toml`.
- Tests cover help contents/scrolling, history cycling/persistence, autosave,
  scratch Save As, terminal directory spawning, and session round trips.

Verification: `cargo test` (329), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

## Post-v0.1.3 — palette hints: done

Landed on 2026-11-22; released in v0.1.4.

- Palette rows are still matched and submitted by command name, but now draw the
  command description and the binding that currently wins on the focused
  surface. `KeyMap::active_binding` resolves the focused scope before global.
- Unbound commands keep a description and no hint, so palette-only actions stay
  visible.
- Tests cover scope precedence and the rendered `[Ctrl+S]` hint.

Verification: `cargo test` (331), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

## Release v0.1.4 — shipped

Shipped as `v0.1.4` on 2026-11-23. This release contains the five post-v0.1.3
passes above:

- Correctness: visible secondary carets and LSP restart/timeouts.
- Daily-driver operations: duplicate, copy path, reveal, workspace and terminal
  rename.
- Performance: viewport-bounded wrapped movement and external-document watching.
- Polish: help overlay, persistent search history, session autosave, scratch
  buffers, and per-workspace terminal directories.
- Palette rows show the active keybinding for the focused surface.

Release record: `cargo fmt --check`, `cargo test` (331), and
`cargo clippy --all-targets --all-features` are clean; `forgeguard gate
--changed` reports no errors or warnings. Version `0.1.4`, tag `v0.1.4`, and the
four platform assets are the release milestone.

## Release v0.1.5 — shipped

Shipped as `v0.1.5` on 2026-11-23. This release contains Git depth, clickable
document tabs, and quit confirmation. `v0.1.x` remains the hardening line;
`v0.2.x` is still reserved for AI/MCP.

### Git depth

- `git::hunks` splits a single-file diff into independently applicable patches.
- `git_stage_hunk` applies one hunk to the index with `git apply --cached`;
  `git_unstage_hunk` reverses it. Untracked files stage as a whole.
- The hunk picker overlay shows the selected hunk, switches hunks with
  `Up`/`Down`, scrolls with `PageUp`/`PageDown`, applies with `Enter`, and closes
  with `Esc`/`q`.
- `git_log` shows the current branch, local branches, and the latest 50 commits
  in the read-only overlay, including the unborn/no-commit state.
- `git_pull` fast-forwards from upstream with `--ff-only` and refuses dirty
  index/worktree or unsaved editor buffers. `git_push` pushes the current branch
  to its upstream with terminal prompts disabled.
- `git_conflicts` lists conflicted files; opening one jumps to the first marker.
  `resolve_conflict_ours`, `resolve_conflict_theirs`, and `resolve_conflict_both`
  replace the block at/after the cursor as one undo step.
- Tests cover a two-hunk stage/unstage round trip, the rendered picker, branch
  and log helpers, the app overlay, a local bare-remote pull/push round trip,
  dirty-pull refusal, conflict parsing, all three resolutions, and opening a
  conflict after a real merge.

Verification: `cargo test` (342), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

Git depth is complete for the v0.1.5 line.

### Navigation polish

- Document tabs inside a pane are clickable. Hit-testing uses the same label
  widths as `ui::pane::render_tabs`; clicking focuses the pane and activates the
  document.
- A mouse regression test switches from a second document back to the first by
  clicking its tab.

Verification: `cargo test` (343), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

### Exit confirmation

- `Ctrl+Q` no longer exits a clean session immediately: it opens a `Quit` /
  `Cancel` prompt. The dirty-session prompt now also includes `Cancel`.
- Tests cover both confirming and cancelling a clean quit.

Verification: `cargo test` (344), `cargo fmt --check`,
`cargo clippy --all-targets --all-features`.

Release record: `cargo fmt --check`, `cargo test` (344), and
`cargo clippy --all-targets --all-features` are clean; `forgeguard gate
--changed` reports no errors or warnings. Version `0.1.5`, tag `v0.1.5`, and the
four platform assets are the release milestone.

## Release v0.1.6 — shipped

Shipped as `v0.1.6` on 2026-11-23. Release engineering, CLI contract hardening,
idle/perf fixes, and test depth on top of v0.1.5; `v0.1.x` remains the hardening
line and `v0.2.x` stays reserved for AI/MCP.

- `ci.yml` runs formatting, `clippy -D warnings`, and tests on Linux, macOS, and
  the declared MSRV; a separate job runs `cargo audit`.
- `release.yml` calls the CI workflow and its build jobs wait for it, so a tag
  cannot publish binaries when formatting, lint, tests, or the audit fail.
- Cargo metadata now declares the MIT license, description, repository, readme,
  and the real MSRV (`1.91`, driven by the dependency set; the old README claim
  of 1.85 was wrong).
- `[profile.release]` uses thin LTO, one codegen unit, and stripped symbols.
  Release binary on this machine: **13.3 MB → 11.4 MB** (about 14% smaller).
- README now has a CI badge and no longer calls the suite “integration tests”.
- Find in Files previews are capped to about 160 chars around the match and reuse
  the current line's char buffer; a 128 KiB single-line regression keeps 1000
  previews under 1 MB instead of storing the whole line 1000 times. Columns are
  char offsets, covered by a Unicode test.
- Terminal scrollback is clamped to `[terminal] scrollback`; modified arrows,
  Home/End, Insert/Delete, PageUp/PageDown, and F1–F12 use xterm CSI modifiers;
  `Ctrl+[ \ ] ^ _` are forwarded; killed shells are reaped with `wait()`.
- CLI parsing uses `pico-args`: unknown options are errors, multiple files open
  as tabs, `FILE:LINE` accepts one file at a 1-based line, and a directory must
  be the only target. `--update`/`--dump-keymap` reject path arguments.
- The event loop draws only when `App.dirty` is set. Idle polling is one second
  without a shell/search/LSP; shell and search retain their faster cadences.
- Workspace watchers are non-recursive and track the root, expanded explorer
  directories, and parent directories of open documents. Unexpanded directories
  are not watched.
- Documents resolve `.editorconfig` `indent_style`/`indent_size`/`tab_width`
  from the workspace root, then language defaults (Go/Makefile tabs), then the
  global `[editor]` settings. Tab insertion, rendering, cursor, and mouse use
  the per-document width.
- Files above 16 MiB are refused on open and reload instead of being read into
  multiple in-memory representations.
- `proptest` exercises random insert/remove/replace sequences and checks that
  undoing everything restores the original and redo restores the final text.
- `assert_cmd` exercises the real binary contract: version, help, unknown-option
  errors, mode/path conflicts, and `--dump-keymap` without a TTY.
- `portable-pty` + `vt100` smoke test opens a file, types `hello`, saves with
  `Ctrl+S`, confirms quit with Enter, and asserts the saved bytes.

Verification: `cargo test` (363 unit + 6 CLI process + 1 PTY smoke = 370),
`cargo fmt --check`, `cargo clippy --all-targets --all-features`,
`cargo +1.91.0 check --locked`, `cargo audit`, and
`cargo build --release --locked`.

Release record: `cargo fmt --check`, `cargo test` (370), and
`cargo clippy --all-targets --all-features` are clean; `cargo audit` reports no
vulnerabilities and `forgeguard gate --changed` reports no errors or warnings.
Version `0.1.6`, tag `v0.1.6`, and the four platform assets are the release
milestone. Release workflow run `45125820386` built and published all four
targets; the aarch64 macOS asset passed its SHA-256 check, reported
`i3-editor 0.1.6`, and a v0.1.5 binary self-updated to v0.1.6 successfully.

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

No unscoped backlog items remain.

## Release v0.1.7 — shipped

Shipped as `v0.1.7` on 2026-11-23. Tab close and overflow scrolling, a
proportional vertical scrollbar, default LSP completion/hover keys, and a
refreshed README recording on top of v0.1.6; `v0.1.x` remains the hardening
line and `v0.2.x` stays reserved for AI/MCP.

- Every document tab renders a clickable `×`; closing a dirty document still
  opens the discard prompt instead of losing the buffer.
- An overflowing document tab strip reserves `‹` / `›` at its edges. Clicking an
  arrow or using the wheel scrolls the strip; the active tab is revealed after
  activation or close, and stays in view until the user scrolls manually.
- Rendering, hit-testing, and scroll clamping share
  `ui::pane::document_tabs` and `tab_strip_geometry`, so a click lands on the
  close button or tab the user sees.
- Files taller than the pane reserve the last body column for a proportional
  vertical scrollbar. Clicking its track jumps, dragging its thumb traverses the
  document, and `ui::pane::scrollbar_geometry` keeps the thumb and mouse mapping
  aligned with `scroll_top`.
- `lsp_completion` defaults to `Ctrl+Space` and `lsp_hover` to `Ctrl+K`; both
  remain overridable under `[keys.editing]`, while `lsp_definition` stays
  palette-only.

Verification: `cargo test` (370 unit + 6 CLI process + 1 PTY smoke = 377),
`cargo fmt --check`, `cargo clippy --all-targets --all-features`, and
`forgeguard gate --changed --output compact`.

Release record: `cargo fmt --check`, `cargo test` (377), and
`cargo clippy --all-targets --all-features` are clean; `cargo audit` reports no
vulnerabilities and `forgeguard gate --changed` reports no errors or warnings.
The refreshed README recording ships with this tag. Version `0.1.7`, tag
`v0.1.7`, and the four platform assets are the release milestone. Release
workflow run `45215664509` built and published all four targets; the aarch64
macOS asset passed its SHA-256 check, reported `i3-editor 0.1.7`, and a v0.1.6
binary self-updated to v0.1.7 successfully.

## Release v0.1.8 — shipped

Shipped as `v0.1.8` on 2026-11-24. The on-demand language-server installer,
simplified command palette, and QTerminal Backspace/Delete compatibility land on
top of v0.1.7; `v0.1.x` remains the hardening line and `v0.2.x` stays reserved
for AI/MCP.

- `lsp_install` (palette) selects a provider from the active file and installs
  its server in the background, with status messages while it works.
- Built-in providers cover Rust, Python, JavaScript/TypeScript, Lua, YAML, and
  Dockerfile/Containerfile. Servers on `PATH` are reused; local installs use
  rustup, an isolated Python virtualenv, npm, or a checksum-pinned Lua release.
- Installer metadata lives in `<config>/i3-editor/lsp.toml`; explicit
  `[lsp.<name>]` config wins, and `filenames` now selects files such as
  `Dockerfile` and `Containerfile` without requiring an extension.
- `lsp_install` refuses to overwrite an explicit configuration and reports
  missing `rustup`, Python, or Node/npm prerequisites.
- Palette names drop the `toggle_` prefix (`comment`, `fold`, `word_wrap`,
  `explorer`, `terminal`, `fullscreen`, `tabbed`, `move_mode`, `hidden_files`).
  `next_theme`/`prev_theme` are gone in favor of `select_theme`, and
  `select_workspace` replaces the per-workspace palette entries.
- Backspace sent as `Ctrl+H` (`0x08`, as QTerminal does) is normalized to
  Backspace; stray modifiers on Backspace/Delete are cleared before lookup.

Verification: `cargo test` (376 unit + 6 CLI process + 1 PTY smoke = 383),
`cargo fmt --check`, `cargo clippy --all-targets --all-features`, and
`forgeguard gate --changed --output compact`.

Release record: `cargo fmt --check`, `cargo test` (383), and
`cargo clippy --all-targets --all-features` are clean; `cargo audit` reports no
vulnerabilities and `forgeguard gate --changed` reports no errors or warnings.
Version `0.1.8`, tag `v0.1.8`, and the four platform assets are the release
milestone. Release workflow run `45332605999` built and published all four
targets; the aarch64 macOS asset passed its SHA-256 check, reported
`i3-editor 0.1.8`, and a v0.1.7 binary self-updated to v0.1.8 successfully.

## Release v0.1.9 — shipped

Shipped as `v0.1.9` on 2026-11-24. Reload hardening, `lsp_uninstall`, pinned
installer packages, and Dockerfile pattern matching land on top of v0.1.8;
`v0.1.x` remains the hardening line and `v0.2.x` stays reserved for AI/MCP.

- `App::new` now stores the real keyboard-enhancement probe result instead of
  `false`; config reload keeps enhanced bindings and no longer reintroduces the
  safe-preset warning.
- Config reload merges installer-managed `lsp.toml` entries after re-reading
  `config.toml`, so theme/config changes do not drop installed language servers
  from the running session.
- Added a merge regression test proving explicit `[lsp.<name>]` still wins over
  the managed entry.
- Added `lsp_uninstall` for installer-managed providers; it removes the managed
  entry and leaves local binaries for manual cleanup.
- Pinned npm and pip package versions, and widened Dockerfile matching to
  `Dockerfile.*`, `Containerfile.*`, and `*.Dockerfile`/`*.Containerfile`.

Verification: `cargo test` (378 unit + 6 CLI process + 1 PTY smoke = 385),
`cargo fmt --check`, `cargo clippy --all-targets --all-features`, and
`forgeguard gate --changed --output compact`.

Release record: `cargo fmt --check`, `cargo test` (385), and
`cargo clippy --all-targets --all-features` are clean; `cargo audit` reports no
vulnerabilities and `forgeguard gate --changed` reports no errors or warnings.
Version `0.1.9`, tag `v0.1.9`, and the four platform assets are the release
milestone. Release workflow run `45413315109` built and published all four
targets; the aarch64 macOS asset passed its SHA-256 check, reported
`i3-editor 0.1.9`, and a v0.1.8 binary self-updated to v0.1.9 successfully.

## Release v0.2.0 — shipped

Shipped as `v0.2.0` on 2026-11-25. The stdio MCP client, guided setup wizard,
persistent trust and recent tools, schema-driven tool forms, logs, and failure
handling are now the release baseline. AI chat and model providers remain
deferred to later `v0.2.x`.

Scope is deliberately narrow: connect to configured MCP servers and let the user
list and call tools explicitly. There is no chat, model provider, automatic edit,
or background agent in this release.

### Agreed behavior

- `mcp` opens the server/tool/log menu; `mcp_add` provides the guided setup
  wizard for the built-in filesystem and everything templates.
- Transport is stdio JSON-RPC 2.0 with newline-delimited messages, not LSP
  `Content-Length` framing.
- Servers start lazily on first use and are killed on exit or workspace-root
  change.
- Configuration lives in `[mcp.<name>]` tables:
  `command`, `args`, optional `cwd`, and an explicit `env` table. The workspace
  root is the default cwd.
- A server trust prompt appears before the first spawn. The trusted identity is
  the hash of command, args, cwd, and env; changing any of them requires trust
  again.
- Tool calls use a schema-driven form with typed fields and path prefill, then a
  per-call confirmation. Results and stderr logs open read-only and never edit a
  buffer or run a shell command implicitly.
- The wizard writes managed `mcp.toml` entries; there is no remote registry or
  automatic package installer.

### Protocol and lifecycle

- Spawn with stdin/stdout pipes; stderr goes to the MCP log ring, not the UI.
- `initialize` → `notifications/initialized` → `tools/list` with pagination.
- Track request ids, bound response size, and time out requests; kill a server
  that violates framing or exits.
- Restart all servers when the workspace root changes; report failures once in
  the status bar and `mcp` logs.
- Sanitize inherited environment variables and never interpolate arguments
  through a shell.

### Delivery checklist

- [x] MCP JSON-RPC framing, request ids, timeouts, and server lifecycle.
- [x] Config schema and validation for `[mcp.<name>]`.
- [x] Mock stdio MCP server fixture for deterministic tests.
- [x] `mcp` palette menu with server/tool lists and a scrollable result overlay.
- [x] Trust prompt with config-hash invalidation and persistent trust hashes.
- [x] Schema-driven argument form (typed fields, required markers, JSON
  fallback), path prefill, per-call confirmation, and server log view.
- [x] `mcp_add` filesystem/everything wizard and persistent recent tools.
- [x] Workspace-root restart and shutdown cleanup.
- [x] README, ARCHITECTURE, and this roadmap updated.

Core landed in `src/mcp`: initialize handshake, `tools/list`, `tools/call`,
request timeouts, stderr/log ring, crash reporting, and `tests/fixtures/mock_mcp.py`
with normal, malformed, crash, silent-timeout, and oversized modes. Config
loading, the `mcp` palette menu, persistent trust hashes, the JSON argument
editor, per-call confirmation, server logs, and the read-only result overlay
now work, with schema-driven argument forms, path prefill, the `mcp_add` wizard,
and recent tools. The suite is 400 (393 unit + 6 CLI + 1 PTY). A manual ignored test
also passed against `@modelcontextprotocol/server-filesystem`: 14 tools listed,
`list_directory` returned `hello.txt`, and the server shut down cleanly.

### Acceptance

- [x] A mock server initializes, lists tools, and returns a tool result in the
  read-only overlay.
- [x] Denying trust prevents spawn; denying a call sends no request.
- [x] Malformed output, timeout, oversized output, and crash are reported without
  blocking the editor.
- [x] No tool result can mutate an editor buffer in v0.2.0.
- [x] `cargo test` (including mock MCP tests), `cargo fmt --check`,
  `cargo clippy --all-targets --all-features`, MSRV 1.91, audit, and gate pass.
- [x] `v0.2.0` tagged after final release review.

### Deferred to v0.2.x

- AI chat and model-provider HTTP clients.
- API key/keyring management.
- Auto-apply diffs and autonomous tool loops.
- MCP HTTP/SSE transport.
- MCP resources/prompts/sampling and installer/registry.

Risk: MCP servers are arbitrary code and return untrusted text. The trust hash,
explicit per-call confirmation, sanitized env, read-only results, and no
auto-apply are the containment boundary for this release.

Release record: `cargo fmt --check`, `cargo test` (400), and
`cargo clippy --all-targets --all-features` are clean; `cargo audit` reports no
vulnerabilities and `forgeguard gate --changed` reports no errors or warnings.
Version `0.2.0`, tag `v0.2.0`, and the four platform assets are the release
milestone. Release workflow run `45569634048` built and published all four
targets; the aarch64 macOS asset passed its SHA-256 check, reported
`i3-editor 0.2.0`, and a v0.1.9 binary self-updated to v0.2.0 successfully.

## Release v0.2.1 — shipped

Shipped as `v0.2.1` on 2026-11-26. The release turns the AI panel into a bounded
MCP/native tool harness while preserving explicit confirmation, trust, audit,
and no-auto-apply boundaries.

### Shipped scope

- AI panel with SSE streaming, non-streaming fallback, retry/backoff, wrapped
  transcript, multiline input, token budget, token meter, copy/clear, and
  horizontal scrolling.
- MCP tool loop with capability labels, allow/deny policy, redacted audit log,
  context/tool-output caps, cancellation, and 8-round limit.
- Native `read_file`/`edit_file`/`write_file`/`search`: workspace boundary,
  snapshot and six-hex line hashes, atomic writes, diff previews, dirty-buffer
  refusal, snapshot-safe undo, and post-edit `cargo check`.
- Explicit file/selection context, opt-in persistent transcript history, and
  opt-in background agents that auto-run read-only tools and pause for approval
  before writes.
- MCP HTTP JSON-RPC transport with JSON/SSE response frames and session IDs,
  resources, prompts, and consent-gated stdio sampling.
- Default provider preset: OpenRouter (`openai/gpt-4o-mini`). Direct keys remain
  local environment/config overrides and are not embedded in the repository.

### Delivery checklist

- [x] AI panel, transcript, streaming, retry, cancellation, and bounded context.
- [x] MCP tools, policy, capability, confirmation, audit, and background loop.
- [x] Native hashline read/edit/write/search, diff preview, undo, cargo check.
- [x] Explicit context, persistent history, MCP HTTP, resources, prompts, sampling.
- [x] Opt-in background agents with read-only auto-run and write approval.
- [x] README, ARCHITECTURE, and this roadmap updated.
- [x] Full suite, fmt, clippy, MSRV 1.91, audit, and gate pass.

### Acceptance

- [x] Tool call, denial, final response, malformed arguments, stale hashes, and
  dirty-buffer refusal are covered by tests.
- [x] MCP stdio and HTTP initialize/list/call, resources, prompts, and sampling
  request handling are covered by mock tests.
- [x] No harness path applies an unconfirmed write or edits a buffer implicitly.
- [x] Background agents require approval before any destructive tool.
- [x] Release version and tag are `0.2.1` / `v0.2.1`; the release workflow
  published four platform assets plus checksums (run `45714031032`).
- [x] Actual provider usage and optional session cost metering in the panel title; hard token/cost caps stop the next request.
- [x] Rich Markdown blocks and inline styling in the AI transcript.
- [x] Follow MCP resource and prompt pagination cursors.
- [x] Add persistent objectives, OS keyring lookup on macOS/Linux/Windows, metadata context index, and write allowlists.
- [x] Add MCP resource templates and pagination.
- [x] Add an opt-in sequential objective queue with automatic cross-session resume.
- [x] Add aligned Markdown tables, MCP prompt argument forms, and objective dependencies.
- [x] Add HTTP server-initiated SSE sampling, bearer-token auth, and custom headers.
- [x] Add task/nested lists, strikethrough, and persisted queue state.
- [x] Add heading levels, escaped punctuation, table alignment, inline table formatting, and nested ordered lists.
- [x] Add opt-in automatic context selection from the path index.
- [x] Attach LSP diagnostics after non-Rust native edits.
- [x] Add MCP resource subscriptions, updates, and list-change notifications.
- [x] Published aarch64 macOS asset passed its SHA-256 check and reports
  `i3-editor 0.2.1`.

### Deferred to later v0.2.x

- Markdown footnotes and nested blockquotes.
- None planned for v0.2.x; remaining items are tracked above.

Risk: MCP servers are arbitrary code and model output is untrusted. Trust hashes,
explicit tool confirmation, capability gating, context/tool budgets, audits,
snapshot validation, and no unconfirmed write remain hard boundaries.

## Release v0.2.2 — shipped

Shipped as `v0.2.2` on 2026-11-26. This release completes the macOS/Linux agent
harness work and adds Windows source support without Windows tests.

### Shipped scope

- Automatic context selection from the workspace path index, opt-in and capped
  by file and character limits.
- Hard session token and cost budgets that stop the next provider request and
  close unresolved tool calls without running them.
- LSP diagnostics appended to non-Rust native edits for open documents with a
  configured language server; five-second timeout.
- MCP HTTP custom headers, bearer-token environment auth, server-initiated SSE
  sampling, resource subscriptions, update notifications, and list-change
  notifications.
- Windows Credential Manager keyring lookup and a Windows CI compile check and
  release target. Windows tests remain skipped.
- Objective queue auto-resume, richer Markdown tables/lists, and the earlier
  v0.2.1 harness remain in this line.

### Delivery checklist

- [x] Full macOS/Linux suite and lint pass.
- [x] Windows source typecheck and clippy pass in CI.
- [x] README, ARCHITECTURE, and ROADMAP updated.
- [x] Version `0.2.2` and tag `v0.2.2` point at the release commit.
- [x] Release workflow builds Linux, macOS, and Windows assets.
- [x] Release workflow run `45732012920` published five assets plus checksums.

### Acceptance

- [x] `cargo test` -> 458 passed, 1 ignored.
- [x] `cargo clippy --all-targets --all-features` -> 0 warnings.
- [x] `cargo fmt --check` -> clean.
- [x] ForgeGuard gate -> 0 findings.
- [x] Automatic context, LSP diagnostic, and MCP subscription behavior covered by tests.
- [x] aarch64 macOS asset passed its SHA-256 check and reports `i3-editor 0.2.2`.
- [x] Both Linux tarball checksums and the Windows zip checksum verified.
- [ ] Windows binary runtime test: intentionally skipped for this release.

## Verification commands

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features
cargo run -- --dump-keymap
```

Last recorded run: `cargo test` → 458 passed (451 unit + 6 CLI process + 1 PTY smoke), `cargo clippy` → 0 warnings.
