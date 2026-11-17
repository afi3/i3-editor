# Specification

`MASTER.md` is the normative specification. This file is the index that maps its
sections to the implementation and records the deltas, so nobody has to diff two
prose documents.

| Requirement | Implementation |
|---|---|
| Ratatui + Crossterm UI | `src/ui/`, `src/terminal.rs` |
| Ropey text buffer | `src/editor/document.rs` |
| Explorer, tabs, panes, workspaces | `src/explorer/`, `src/state.rs`, `src/workspace/` |
| Recursive weighted layout | `src/workspace/layout.rs` |
| Centralized read-only render | `src/ui/mod.rs` (`render` takes `&AppState`) |
| Command-based mutation | `src/command.rs`, `src/app/dispatch.rs` |
| TOML configuration | `src/config/mod.rs` |
| Keyboard configuration | `src/input/keymap.rs`, `src/input/presets.rs` |
| Prompts (`MASTER` 5.3) | `src/input/prompt.rs`, `src/app/prompt.rs` |
| Undo model (`MASTER` 5.3.1) | `src/editor/history.rs` |
| Terminal cleanup (`MASTER` 12.13) | `src/terminal.rs` |

## Deltas from the text

1. **Split geometry is computed in `workspace::layout::split_area`, not by
   `Layout` constraints.** Reason: minimum pane size must be guaranteed while
   distributing leftover cells, which `Constraint::Fill` cannot express. Ratatui
   still draws every region. See `ARCHITECTURE.md`.
2. **Key scopes are `global`, `editing`, `explorer`.** `MASTER`'s older
   `[keys.pane]` example was replaced, because splits are global commands. The
   loader accepts exactly these three sections; unknown sections inside `[keys]`
   are ignored by serde's default behaviour.
3. **Accepted but not applied configuration keys** are listed in `README.md`
   under known limitations.

## Non-negotiables that hold

- Rendering never mutates state.
- Every mutation flows through a `Command`.
- A dirty document is never discarded without an explicit confirmation.
- No pane can shrink below `MIN_PANE_ROWS = 4` / `MIN_PANE_COLS = 10`.
- Terminal state is restored on normal exit, panic, and SIGINT/SIGTERM/SIGHUP.
