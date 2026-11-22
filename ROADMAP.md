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

## Planned release v0.1.3 — all remaining features

MVP shipped as `v0.1.2` on 2026-11-22. Every feature previously spread across
v0.1.3–v0.1.9 is now planned for a single release, **v0.1.3**. There are no
intermediate tags in this cycle.

Policy:

- `Cargo.toml` stays at the released version (0.1.2) until every workstream
  below passes acceptance. Do not bump, tag, or push a release before then.
- Commit per workstream and keep `main` green. The single release tag is the
  only external milestone; individual features are reviewed through commits.
- If a workstream has to ship early for safety, that requires an explicit
  decision to split the release; the default is one tag.
- Keep this section current: tick the progress list and update the test count as
  work lands, so a new session does not rescan the codebase.

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
- [ ] 2. Search and replace parity
- [ ] 3. Navigation, workspace roots, recovery
- [ ] 4. LSP MVP
- [ ] 5. Git integration
- [ ] 6. Editing model expansion
- [ ] 7. Lua extension host

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

- Regex mode plus literal mode, whole-word, and a case toggle.
- Apply the same options to Find and Find in Files.
- Replace in Files with preview, per-file confirmation, and undo-safe writes
  (atomic save path already exists).
- Report match counts and truncation consistently in prompt titles.

Touches: `src/app/search.rs`, `src/project_search.rs`, `src/app/project_search.rs`,
`src/input/prompt.rs`, `src/ui/overlay.rs`, `src/command.rs`.

Acceptance: regex and literal tests including invalid patterns, whole-word at
boundaries, replace preview does not write before confirmation, and a replace
across two files is verified by reading them back.

### 3. Navigation, workspace roots, recovery

- Outline / go-to-symbol per document from the existing tree-sitter tree.
- Tree-sitter folding for functions, blocks, and Markdown sections.
- Per-workspace explorer root and watcher, so workspaces are independent
  environments instead of sharing one folder.
- Crash recovery: a swap/autosave file for dirty buffers, replayed on next start
  with overwrite/discard/reload choices.

Touches: `src/syntax/tree.rs`, a new `src/symbols/` or `src/outline.rs`,
`src/explorer/mod.rs`, `src/watch.rs`, `src/state.rs`, `src/workspace/*`,
`src/session.rs`, `src/app/mod.rs`, `src/ui/pane.rs`.

Acceptance: outline jump lands on the symbol line, folding hides the expected
range, switching workspaces changes the explorer root and quick-open list,
crash recovery restores a dirty buffer after a synthetic exit without save.

### 4. LSP MVP

- Async LSP client with per-server lifecycle and document sync.
- Diagnostics surfaced in the status bar and in the gutter.
- Completion popup, hover, and go-to-definition.
- Config table for server commands and file-type mapping.

Touches: new `src/lsp/`, `src/app/mod.rs` event loop, `src/ui/overlay.rs`,
`src/ui/pane.rs`, `src/config/mod.rs`, keymap/palette commands.

Acceptance: a fake or real server fixture produces a diagnostic, completion
popup, hover text, and a definition jump in tests or a scripted pty check.

Risk: largest architectural change; write the client/lifecycle design before
implementation. Keep completion optional so a missing server never blocks edits.

### 5. Git integration

- Status markers in the gutter (modified/added/deleted lines).
- Read-only diff view for the active file and a staged/unstaged list.
- Stage, unstage, and commit from the palette.
- Shell out to `git`; do not add a libgit2-style dependency for MVP.

Touches: new `src/git/`, `src/ui/pane.rs` gutter, `src/app/mod.rs` refresh
trigger, palette commands, config for the git binary.

Acceptance: fixture repository shows correct markers, diff view opens without
modifying files, staging changes the index, and every command reports git errors
in the status bar.

### 6. Editing model expansion

- Multi-cursor and column selection, including copy/cut/undo behavior.
- Soft word wrap with correct cursor movement, hit-testing, and scroll.
- Line-ending tools: convert LF/CRLF for the active document, with the save path
  and EOL status staying consistent. Other encodings remain rejected and
  documented; UTF-8 with BOM is the only encoding addition considered.

Touches: `Pane` selection model in `src/state.rs`, `src/app/cursor.rs`,
`src/app/editing.rs`, `src/app/mouse.rs`, `src/ui/pane.rs`,
`src/editor/document.rs`, `src/editor/text.rs`.

Acceptance: multi-cursor edit/undo tests, wrapped-cursor movement and click
tests, CRLF conversion round-trip, and non-UTF-8 rejection with a clear message.

Risk: multi-cursor and wrap change invariants used by search, copy, mouse, and
undo; land them behind small, tested steps rather than one change.

### 7. Lua extension host

Goal: add a sandboxed extension layer on top of the Rust core. Lua language
support already shipped in workstream 1; this part embeds the VM and host only.

- Embed `mlua` (vendored Lua 5.4) and load `<config>/i3-editor/plugins/*.lua`.
- API v1: `commands.register`, `keymap.set`, `events.on` (`open`, `save`,
  `change`, `focus`), `statusline.register`, and read-only editor queries (path,
  language, cursor, selections). `selections` is always a list.
- Run callbacks on the UI thread with a time budget and no blocking I/O; long
  work must dispatch a Rust command or task. Guard reentrancy with a depth limit.
- Sandbox by default: no `io`, `os`, or `package`; capability opt-in for file and
  process access. Isolate errors per plugin and report `path:line` through
  `messages`; disable a plugin after repeated failures.
- Watch the plugin directory; reloading a plugin unregisters its commands,
  keymaps, hooks, and statusline segments first.
- Keep TOML as the validated source of truth. `init.lua` overlay remains out of
  scope.
- Demo plugins (keymap/command, save hook, statusline) double as fixtures.

Touches: new `src/lua/` (host + API), `src/command.rs` (registry hooks),
`src/app/mod.rs` (dispatch/event integration), `src/watch.rs`,
`src/config/mod.rs`, `src/ui/mod.rs` (statusline segments), `Cargo.toml`.

Acceptance: a fixture script registers a command and keymap, a save hook writes a
marker, a statusline segment renders, `editor.selections()` returns a list, a
broken script reports `path:line` without crashing, reload removes old handlers,
and a budget overflow is reported and stopped.

Risk: API churn and sandbox escapes. Version the API (`api_version`) from the
first release, keep host capabilities narrow, and do not call Lua per frame.

Dependencies: `mlua = "0.12"` with `lua54` and `vendored`. `tree-sitter-lua`
already lands in workstream 1.

### Overall acceptance for v0.1.3

- Every workstream acceptance above passes.
- `cargo fmt --check`, `cargo check`, `cargo test`,
  `cargo clippy --all-targets --all-features` are clean.
- `forgeguard gate --changed` reports no errors or warnings.
- README, ARCHITECTURE, and this roadmap reflect the shipped features.
- Only then: bump to 0.1.3, tag `v0.1.3`, push, and verify four release assets.

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

Last recorded run: `cargo test` → 248 passed, `cargo clippy` → 0 warnings.
