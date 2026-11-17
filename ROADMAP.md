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

## Phase 8 — MVP stabilization: partially done

Delivered:

- Config loading with per-section fallback and warnings.
- Keymap presets and capability handling.
- Unicode width/grapheme handling with tests.
- Error handling through a single `Error` type; status-bar messages.
- 56 unit and integration tests, `TestBackend` render tests.
- `README.md`, `ARCHITECTURE.md`, `ROADMAP.md`.

Not delivered yet:

- `editor.word_wrap`, `appearance.theme`, `workspace.auto_save_session` are
  parsed but unused. Either implement them or remove the keys.
- Find highlights only the current match; there is no match list UI.
- Undo does not clear the modified flag when the buffer returns to the saved
  state (needs a save revision counter).
- No session persistence across restarts.
- Future-phase features remain out of scope: Tree-sitter highlighting, Nucleo
  fuzzy scoring, Notify file watching.

## Verification commands

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features
cargo run -- --dump-keymap
```

Last recorded run: `cargo test` → 56 passed, `cargo clippy` → 0 warnings.
