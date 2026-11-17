# Data model

Types as implemented. Invariants are enforced by the constructors and the tests
listed in `ARCHITECTURE.md`.

## Identifiers

`DocumentId(u64)`, `PaneId(u64)`, `WorkspaceId(u64)`. Monotonic counters in
`state::Ids`; ids are never reused, so a stale reference cannot silently point at
a new object.

## Document — `src/editor/document.rs`

| Field | Meaning |
|---|---|
| `id: DocumentId` | Stable identity |
| `path: Option<PathBuf>` | `None` for an unsaved buffer |
| `rope: Rope` | Text, always LF internally |
| `dirty: bool` | Modified since load/save |
| `eol: Eol` | `Lf` or `Crlf`, applied on write only |
| `history: History` | Undo/redo log |

Invariants: line endings are normalized on load and restored on save; the rope
stores tabs literally, expansion is a render concern.

## History — `src/editor/history.rs`

`Edit::Insert { char_idx, text }` / `Edit::Remove { char_idx, text }` and
`Batch { edits, cursor_before, cursor_after, scroll_before }`.

- `char_idx` is a Rope char index, never a byte offset.
- Undo applies inverses in reverse order; redo reapplies forward.
- Insert runs coalesce while consecutive, contiguous, newline-free, and less than
  300 ms apart. Any other operation breaks the run and clears the redo stack.

## Pane — `src/state.rs`

`tabs: Vec<DocumentId>`, `active: Option<DocumentId>`, `cursor: usize` (char
index), `anchor: Option<usize>` (selection anchor), `scroll_top: usize`,
`scroll_left: u16` (cells), `target_col: Option<usize>` (vertical-motion goal).

Invariant: `active` is always present in `tabs` when a tab is open; selection is
`min(anchor, cursor)..max(anchor, cursor)`.

## Layout — `src/workspace/layout.rs`

`LayoutNode::Pane(PaneId)` or
`LayoutNode::Split { axis, mode, active, weights, children }`.

- `mode: Tiled | Tabbed`. Tiled shares the area along `axis`; Tabbed shows only
  `children[active]` and reserves `TAB_BAR_ROWS` (1) for a tab strip.
- `weights.len() == children.len()`, each `1..=100`.
- Tiled children count is 2..=`MAX_CHILDREN` (4); tabbed children count is
  2..=`MAX_TABS` (8). Both collapse when one child remains.
- `active < children.len()`; removal and collapse re-clamp it.
- A split is only created when every resulting pane can satisfy the minimum on
  the split axis. For tabbed containers the minimum is `max(child)` plus the bar
  row on `TopBottom`.
- `SplitAxis::LeftRight` maps to `Direction::Horizontal`,
  `TopBottom` to `Direction::Vertical`.
- `TabStrip { path, rect, leaves, counts, active }` is derived per frame by
  `tab_strips`, never stored in the tree.

## Workspace — `src/workspace/mod.rs`

`id`, `name`, `layout: LayoutNode`, `focus: PaneId`. `focus` remembers the pane
to restore when the workspace becomes active again.

`WorkspaceManager { workspaces: Vec<Workspace>, active: usize }` — cannot close
its last workspace.

## Application state — `src/state.rs`

| Field | Purpose |
|---|---|
| `should_quit` | Loop exit |
| `explorer` | Tree rows, selection, filters |
| `documents: HashMap<DocumentId, Document>` | Buffers |
| `panes: HashMap<PaneId, Pane>` | Viewports |
| `workspaces` | Layouts and active index |
| `focus: Explorer \| Pane(PaneId)` | Input target |
| `mode: Normal \| Prompt(PromptState)` | Input routing |
| `keymap: KeyMap` | Three binding scopes |
| `ui: UiState` | Rect cache, status message |
| `config: AppConfig` | Parsed TOML |
| `ids: Ids` | Id counters |
| `warnings: Vec<String>` | Config and keymap problems |
| `clipboard: String` | Internal clipboard |
| `search: Option<Search>` | Query, match positions, current index |
| `fullscreen: Option<PaneId>` | Pane shown over the whole pane area |
| `moving: bool` | Grab mode: arrows move the focused pane |

## Prompt — `src/input/prompt.rs`

`kind: PromptKind`, `fields: Vec<String>` (0, 1, or 2), `active`,
`cursor` (char index), `candidates` + `filtered` + `selected`, `target` path, and
`document` id for the discard prompt. Field count is a property of the kind, so a
confirmation prompt has no text input.

## Keymap — `src/input/keymap.rs`

`KeyCombo { code: KeyCode, mods: KeyModifiers }` normalized so that uppercase
characters carry `SHIFT` and `BackTab` drops it. `KeyMap { global, editing,
explorer }` maps combos to `Command`. Resolution: prompt, focused scope, global.
