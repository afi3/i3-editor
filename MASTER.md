# MASTER PROMPT — i3-editor

## 1. Project Identity

You are a senior Rust engineer and software architect.

Your task is to design and implement **i3-editor**, a modern terminal-based text and code editor written entirely in Rust.

i3-editor combines the simplicity of terminal editors such as Nano, the flexibility of Vim-inspired editing workflows, and the workspace management experience of modern IDEs.

The application must run entirely inside a terminal. Do not use a graphical windowing framework.

The UI must be built using Ratatui.

Your responsibility is to build a functional, maintainable, modular, and extensible application, not merely a prototype or UI mockup.

## 2. Core Vision

i3-editor is a keyboard-first TUI editor with the following characteristics:

- Modern editing experience (non-modal by default).
- Persistent file explorer on the left.
- Multiple documents and document tabs.
- Multiple workspaces.
- Vertical and horizontal editor splitting.
- Nested split layouts.
- Independent cursor and scroll position per pane.
- General-purpose text and code editing.
- Terminal-native interface.
- Configurable keyboard shortcuts and appearance.

The application should be usable for both source code editing and ordinary text editing.

## 3. Technology Stack

Use the following technologies:

| Component | Technology |
|---|---|
| Programming Language | Rust |
| TUI Framework | Ratatui |
| Terminal Backend | Crossterm |
| Text Buffer | Ropey |
| Syntax Parsing | Tree-sitter (future phase) |
| Configuration | Serde + TOML |
| Error Handling | Anyhow + Thiserror |
| File Ignore | Ignore |
| Fuzzy Finder | Nucleo (future phase) |
| File Watching | Notify (future phase) |
| Unicode Width | Unicode-width |
| Grapheme Segmentation | Unicode-segmentation |

Use stable Rust and compatible stable dependency versions.

Do not introduce unnecessary dependencies. `unicode-width` and `unicode-segmentation` are not optional: correct cell rendering and grapheme-aware cursor movement are impossible without them.

Versions verified against crates.io on 2026-11-17:

| Crate | Version | Note |
|---|---|---|
| ratatui | 0.30.2 | Re-exports `ratatui::crossterm` |
| crossterm | 0.29.0 | Use the Ratatui re-export; never add a second copy |
| ropey | 1.6.1 | No undo, no display-width helpers |
| ignore | 0.4.34 | |
| nucleo | 0.5.0 | |
| notify | 8.2.2 | |
| unicode-width | 0.2.3 | |
| unicode-segmentation | 1.13.3 | |

Two crossterm versions compiled into one binary produce confusing trait-mismatch errors. Depend on Ratatui only and import the re-export.

Before using a library API, verify its actual available version and documentation.

## 4. Main Interface

The application must have the following layout:

### 4.1 File Explorer

- Positioned on the left.
- Default width: 20% of terminal width.
- Expandable and collapsible.
- Directory tree navigation.
- Expand/collapse directories.
- Open files.
- Create files and directories.
- Rename files and directories.
- Delete files and directories.
- Optional hidden file visibility.
- Respect `.gitignore` where practical.
- Mouse: click selects a row, double-click opens it, wheel moves the selection.

### 4.2 Main Editor Area

The remaining 80% of terminal width is reserved for the editor.

Each editor pane must support:

- Document tab bar.
- Text editing.
- Cursor navigation.
- Line numbers.
- Scrolling.
- Modified document indicator.
- Status information.
- Independent cursor and viewport state.
- Fullscreen toggle for the focused pane; the layout tree is preserved while one
  pane occupies the whole editor area.
- Mouse: click places the cursor, Shift+click extends the selection, and the
  wheel scrolls the viewport without moving the cursor.

### 4.3 Workspace Tab Bar

Workspace tabs must be separate from document tabs.

A workspace represents an independent editing environment.

Example:

Workspace 1: Backend Development
- main.rs
- config.rs
- Two vertical panes.

Workspace 2: Documentation
- README.md
- architecture.md
- One editor pane.

Switching workspaces must preserve their document tabs and layout.

### 4.4 Status Bar

Display relevant information:

- Current document name.
- Current line and column.
- Total lines.
- File encoding.
- Modified state.
- Current focus.
- Application status messages.

## 5. Critical Architecture Decisions

These decisions are mandatory.

### 5.1 Separate Document, Pane, and Workspace

Never combine these three concepts.

Document:
- Represents the actual file and its content.
- Owns the Ropey text buffer.
- Tracks file path and modified state.
- Can be referenced by multiple panes.

Pane:
- Represents an editor viewport.
- References a DocumentId.
- Owns its cursor position.
- Owns its scroll position.
- Owns its document tabs.
- Has independent viewport dimensions.

Workspace:
- Represents a collection of panes.
- Owns its recursive layout tree.
- Tracks the currently focused pane.

Workspace Manager:
- Manages multiple workspaces.
- Tracks the active workspace.

A document may be displayed in multiple panes without duplicating its underlying text buffer.

### 5.2 Recursive Layout Tree

Implement split management using a recursive tree.

Use the following conceptual model:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum SplitAxis {
    /// Children laid out left to right.
    LeftRight,
    /// Children laid out top to bottom.
    TopBottom,
}

enum LayoutNode {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        mode: SplitMode,        // Tiled | Tabbed
        active: usize,          // visible child when Tabbed
        /// `weights.len() == children.len()`, each weight in `1..=100`.
        weights: Vec<u16>,
        children: Vec<LayoutNode>,
    },
}
```

`SplitMode::Tiled` shares the area along `axis`. `SplitMode::Tabbed` gives the
children the same area, shows only `children[active]`, and reserves one row for a
container tab strip. A split with a single child collapses regardless of mode.

Definitions:

- `LeftRight`: divide available space into left and right regions. Maps to `ratatui::layout::Direction::Horizontal`.
- `TopBottom`: divide available space into top and bottom regions. Maps to `ratatui::layout::Direction::Vertical`.
- `weights`: relative distribution with one entry per child, not a single percentage. A split with three children carries three weights.

Never name these axes `Horizontal` and `Vertical`. Ratatui's `Direction::Horizontal` means left to right, while "horizontal split" is commonly read as top to bottom. The renamed axes keep the mapping literal and remove a whole class of transposed-layout bugs.

Requirements:

- Support nested splits.
- Support multiple child panes, at least three per split, with independent weights.
- Support closing panes.
- Support moving panes: swap a pane with its geometric neighbour without
  changing the tree shape or the split weights.
- Support resizing splits.
- Support tabbed containers: toggle the innermost split around the focused pane
  between tiled and tabbed, switch tabs by keyboard and mouse, and add a tab when
  splitting inside a tabbed container.
- Automatically collapse redundant split nodes.
- Never leave an invalid empty layout.
- Maintain stable PaneId references.
- Handle terminal resizing correctly.

Rendering rules:

- Convert `weights` to one `Constraint::Fill(w)` per child and render with Ratatui Layout.
- Do not hardcode pane coordinates.
- Compute the pane rect map once per frame from the active workspace layout, store it in `UiState`, and reuse it for rendering, mouse hit-testing, cursor placement, and scroll clamping.
- Recompute the map only when the layout tree or the terminal size changes (`layout_dirty`).

Minimum size rules:

- A pane requires at least `MIN_PANE_ROWS = 4` and `MIN_PANE_COLS = 10`: tab bar, gutter, borders, and one text row.
- A split may only be created when both resulting subtrees satisfy the minimum along the split axis.
- Every subtree exposes `min_size(axis) -> u16`, computed bottom-up, so a resize clamp is `min(left.min_size(axis), right.min_size(axis))` per side.
- Terminal shrink below the minimum must clamp, not divide by zero: every viewport calculation uses saturating arithmetic and a text height of `max(1, ...)`.
- A pane smaller than the minimum hides its tab bar and borders and still renders one text row. Its text area is never an empty `Rect`.

Separator resize rules:

- Resizing the separator between children `i` and `i + 1` moves `weights[i]` in one direction and `weights[i + 1]` in the other by the same amount, so the sum stays constant.
- Clamp so both neighbours keep their `min_size`.
- Weights stay in `1..=100`. A zero weight is a bug, not an empty pane.

Collapse rules:

- A `Split` with one child collapses into that child.
- A `Split` with zero children is removed from its parent.
- A `Workspace` whose root collapses to nothing is replaced with a single empty `Pane` carrying a fresh `PaneId`.
- Closing a pane removes its `LayoutNode`; the pane's tabs are reassigned to a sibling pane or closed with the pane, and the `Pane` entry is dropped afterwards.
- Closing a pane never drops a dirty `Document`. The document stays registered and unreferenced until the user saves, discards, or reopens it in a pane.

Use Ratatui Layout for rendering calculated regions.

### 5.3 State Management

Use a centralized application state.

Conceptual model:

```rust
struct AppState {
    should_quit: bool,
    explorer: ExplorerState,
    documents: HashMap<DocumentId, Document>,
    panes: HashMap<PaneId, Pane>,
    workspaces: WorkspaceManager,
    focus: Focus,
    mode: InputMode,
    keymap: KeyMap,
    ui: UiState,
    config: AppConfig,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Explorer,
    Pane(PaneId),
}

enum InputMode {
    /// Keys reach the focused surface through the keymap.
    Normal,
    /// One prompt owns every key except the global escape hatches.
    Prompt(PromptState),
}
```

Adapt the actual implementation when necessary, but preserve the architectural responsibilities.

`PromptState` is the single implementation behind every dialog: quick open, find, replace, save as, new file, new directory, rename, delete confirmation, and discard confirmation. A prompt carries its kind, its input text and cursor, its candidate list, and its submitted result. "Non-modal editing" describes the default editing experience; it does not permit a dialog to be improvised inside a rendering widget.

Key routing is fixed and applies to every `KeyEvent`:

1. Non-key events (resize, mouse) always apply.
2. If `mode` is `Prompt`, the prompt consumes the key. Only `Esc` and the quit binding escape it.
3. Otherwise the global keymap applies: quit, save, explorer toggle, quick open, command palette, workspace switch.
4. Otherwise the binding for the focused surface applies (`Focus::Pane` or `Focus::Explorer`).
5. Unmatched keys are dropped. They never fall through to the editor by accident.

Rendering must be read-only.

All application mutations must happen through event handlers, commands, or explicit state operations. Event handlers translate input into `Command` values; only `Command::apply(&mut AppState)` mutates state. Command handlers return `Result`, and a failure pushes a message into `UiState` instead of aborting the loop.

Separate `documents` and `panes` maps are also what makes the borrow checker cooperate. Destructure to mutate a pane and its document together:

```rust
let AppState { documents, panes, .. } = &mut self.state;
let pane = panes.get_mut(&pane_id).ok_or(Error::UnknownPane(pane_id))?;
let doc = documents
    .get_mut(&pane.document_id)
    .ok_or(Error::UnknownDocument(pane.document_id))?;
```

Do not reach for `RefCell` or `Rc` to solve this. Field destructuring is the pattern; interior mutability hides the invariant that only one command mutates at a time.

#### 5.3.1 Edit history

Ropey provides no undo. The editor owns an explicit operation log.

```rust
enum Edit {
    Insert { char_idx: usize, text: String },
    Remove { char_idx: usize, text: String },
}

struct Batch {
    edits: Vec<Edit>,
    cursor_before: (usize, usize),
    scroll_before: (u16, u16),
}

struct History {
    undo: Vec<Batch>,
    redo: Vec<Batch>,
}
```

Rules:

- Undo applies the inverse of each `Edit` in reverse order; redo reapplies the batch.
- A `Batch` groups edits that share one user intent. Consecutive plain-character insertions coalesce until a gap of about 300 ms, a cursor move, a newline, or a selection change.
- Each batch restores cursor and scroll to `cursor_before` and `scroll_before` on undo.
- The redo stack is cleared by the first new edit after an undo.
- `char_idx` is a Ropey char index. Never a byte offset, never a line/column pair.

## 6. Project Structure

Use a modular structure similar to:

```text
i3-editor/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── AGENTS.md
├── SPEC.md
├── ARCHITECTURE.md
├── DATA-MODEL.md
├── ROADMAP.md
│
├── src/
│   ├── main.rs
│   ├── app.rs
│   ├── state.rs
│   ├── event.rs
│   ├── command.rs
│   ├── error.rs
│   │
│   ├── terminal/
│   ├── ui/
│   ├── editor/
│   ├── workspace/
│   ├── explorer/
│   ├── input/
│   │   ├── keymap.rs
│   │   ├── prompt.rs
│   │   └── router.rs
│   └── config/
│
└── tests/
```

Each module should have a clear responsibility.

Avoid putting the entire implementation into `main.rs`.

Expand the folder structure when necessary.

## 7. Editor Behavior

The default editing experience is modern, non-modal.

Required functionality:

- Direct text insertion.
- Backspace and delete.
- Cursor movement.
- Home and End.
- PageUp and PageDown.
- Text selection.
- Copy, cut, and paste.
- Undo and redo.
- Find and replace.
- Auto indentation.
- Configurable tab size.
- Line numbers.
- Horizontal and vertical scrolling.
- Grapheme-aware cursor movement.
- Bracketed paste input.
- Internal clipboard with best-effort OSC 52 copy.

Use Ropey for text storage and editing operations.

Do not use a simple String as the primary document buffer.

Ensure that character indices, line/column positions, Unicode characters, tab widths, and terminal cell widths are handled correctly:

- Cursor motion and deletion operate on grapheme clusters via `unicode-segmentation`, not on `char` values.
- Column arithmetic and rendering use `unicode-width`, so CJK characters and emoji occupy two cells.
- The document stores char indices. Byte offsets and cell columns are derived at the boundary, never stored as the cursor's primary representation.
- Tab width is configuration expanded at render time; the buffer keeps the literal tab byte.

Enable bracketed paste on startup and disable it on cleanup, so a multiline paste arrives as one paste event instead of a stream of keystrokes that auto-indent would corrupt.

Preserve the file's original line ending on save, or normalize explicitly and say so in the status message. Never silently rewrite a CRLF file as LF.

For syntax highlighting, prepare the editor architecture for Tree-sitter integration, but do not make advanced language support a blocker for the initial MVP.

## 8. Default Keyboard Shortcuts

Implement configurable keyboard mappings. Bindings are data, not `match` arms buried in the event loop.

### 8.1 Binding model

```toml
[keys]
preset = "default"            # "default" | "safe"

[keys.global]
quit = "ctrl+q"
save = "ctrl+s"
save_as = "f2"
quick_open = "ctrl+p"
command_palette = "f1"
split_left_right = "f3"
split_top_bottom = "f4"
close_pane = "f6"

[keys.editing]
undo = "ctrl+z"
redo = "ctrl+y"
find = "ctrl+f"
replace = "ctrl+r"

[keys.explorer]
rename = "r"
delete = "d"
```

Rules:

- Every action has a stable name; a binding maps `KeyCombo -> Action`.
- An unknown action name or an unparsable combo is a configuration error reporting file, line, and the offending text. The app starts with the built-in default for that entry and shows the message, so one typo does not brick startup.
- Two actions bound to the same `KeyCombo` in the same scope is a conflict: report it and keep the first, never pick silently.
- Scopes resolve top-down: prompt, then focused surface, then global. A user binding may override a pane binding; it may not override the quit binding.
- `preset = "safe"` drops every binding that needs an enhanced keyboard protocol. It must remain fully usable, with the command palette as the escape hatch for anything not bound.

### 8.2 Terminal capability limits

Terminals cannot represent every modifier combination. These are facts about the wire protocol, not implementation shortcuts:

| Combination | Reality | Rule |
|---|---|---|
| `Ctrl+Shift+<letter>` | Same byte as `Ctrl+<letter>` without the Kitty keyboard protocol (CSI-u) | Never a default binding on its own |
| `Ctrl+H` | Same byte as `Backspace` (`0x08`) | Never bind |
| `Ctrl+I`, `Ctrl+M`, `Ctrl+J` | Same bytes as `Tab`, `Enter`, `LineFeed` | Never bind |
| `Ctrl+2`, `Ctrl+3`, `Ctrl+4`..`Ctrl+8` | Emit `NUL`, `ESC`, or other control bytes | Never use for workspace switching |
| `Ctrl+1`..`Ctrl+9` | Unreliable outside Kitty/CSI-u | Best-effort only; every action reachable another way |
| `Alt+<letter>`, `Alt+Arrow` | macOS terminals send Unicode text or `Esc`-prefixed sequences by default | Opt-in preset only, never the sole path |
| `F1`..`F12` | macOS laptops require `Fn` unless function keys are remapped | Secondary binding, never the only path |
| `Ctrl+Arrow`, `Ctrl+Shift+Arrow` | Reliable CSI sequences (`\x1b[1;5D`, `\x1b[1;6D`) | Safe for pane navigation |
| `Ctrl+\`, `Ctrl+_`, `Ctrl+@`, `Ctrl+]`, `Ctrl+^` | Own control bytes, delivered intact in raw mode | Safe |

Startup capability probe:

- Query `supports_keyboard_enhancement()` before enabling enhancements.
- If supported, push Kitty keyboard enhancement flags, and pop them during cleanup on every exit path.
- If not supported, fall back to the `safe` preset and emit one status message naming the actions whose default binding is unavailable.
- Log the resolved binding table once at startup in debug mode; this is the cheapest way to debug "my shortcut does nothing".

Test the resolved bindings against the actual event stream: a mapping is only correct if the byte sequence the terminal sends for that key resolves to the expected `Action`.

### 8.3 Clipboard

Copy, cut, and paste are split between the internal clipboard and the terminal:

- Always write the internal clipboard first. It is the fallback when OSC 52 is unsupported, disabled, or truncated.
- Best-effort write OSC 52 so the text also reaches the system clipboard. An oversized payload is a warning in the status bar, not an error.
- Paste arrives through bracketed paste events. Applying per-line auto-indent to a paste is a bug.
- `Ctrl+C` copies only when a selection exists. With no selection it cancels the active prompt or reports "nothing to copy"; it does not silently do nothing. Preserve the terminal-wide expectation that `Ctrl+C` means interrupt.
- `Ctrl+X` behaves like `Ctrl+C` for the selection, then deletes it.
- `Ctrl+V` pastes the internal clipboard. The terminal's own paste arrives as a bracketed paste event, which must not be double-inserted.

### 8.4 Defaults

These defaults avoid every unreliable combination in 8.2.

#### Global

| Shortcut | Action |
|---|---|
| Ctrl+Q | Quit |
| Ctrl+S | Save |
| F2 | Save As |
| Ctrl+O | Open File |
| Ctrl+N | New File |
| Ctrl+W | Close Document |
| Ctrl+B | Toggle Explorer |
| Ctrl+P | Quick Open |
| F1 | Command Palette |
| Ctrl+G | Go to Line |
| Ctrl+Tab | Next Document |
| BackTab | Previous Document |

#### Workspace and Split

| Shortcut | Action |
|---|---|
| Ctrl+1 to Ctrl+9 | Switch Workspace (best-effort, always available via palette) |
| F3 | Split Left/Right |
| F4 | Split Top/Bottom |
| F6 | Close Pane |
| F9 | Toggle Fullscreen Pane |
| F10 | Grab Mode (arrows move the focused pane) |
| F12 | Toggle Tabbed Container |
| Ctrl+PageUp / Ctrl+PageDown | Previous / next tab in the container |
| Ctrl+Shift+Arrow | Navigate Panes |
| F7 | New Workspace |
| F8 | Close Workspace |

#### Editing

| Shortcut | Action |
|---|---|
| Ctrl+Z | Undo |
| Ctrl+Y | Redo |
| Ctrl+A | Select All |
| Ctrl+C | Copy (selection only) |
| Ctrl+X | Cut (selection only) |
| Ctrl+V | Paste |
| Ctrl+F | Find |
| Ctrl+R | Replace |
| Ctrl+D | Duplicate Line |

Test shortcut compatibility with Crossterm and supported terminal emulators.

Do not assume every terminal can distinguish every key combination.

## 9. Configuration

Use TOML.

Default configuration location:

`~/.config/i3-editor/config.toml`

Example:

```toml
[editor]
tab_size = 4
insert_spaces = true
line_numbers = true
word_wrap = false
auto_indent = true

[explorer]
visible = true
width = 20
show_hidden = false

[workspace]
default_layout = "vertical"
auto_save_session = false

[appearance]
theme = "default"
show_status_bar = true
show_document_tabs = true
show_workspace_tabs = true

[keys]
preset = "default"
```

Key bindings use the format defined in 8.1 and live under `[keys.*]`. The example above shows the preset only; a full binding table is optional.

Support sensible defaults when the configuration file does not exist.

Invalid configuration must not brick startup. A malformed entry produces a warning that names the file, line, and entry, falls back to the built-in default for that entry alone, and keeps the rest of the user configuration.

An invalid `[keys]` combination is the highest-risk case: it must be reported through the same channel, because a silently dropped binding looks like a broken application.

## 10. Development Roadmap

Implement the project incrementally. Every phase leaves the application runnable.

### Phase 1 — Terminal Foundation and Pane Skeleton

- Initialize Rust project.
- Integrate Ratatui and Crossterm.
- Implement terminal setup and cleanup on every exit path, including panics and signals.
- Implement event loop.
- Implement `AppState` with a single `Pane`, a layout tree holding one `LayoutNode::Pane`, and one workspace.
- Compute the pane rect map once per frame.
- Render explorer region, pane region, workspace tab bar, and status bar.
- Handle terminal resizing, including shrinking below the minimum pane size.

Definition of done: `cargo run` renders; resizing to 40×10 and back does not panic; Ctrl+Q restores the terminal; a forced panic still restores it.

### Phase 2 — Keymap, Input Routing, Prompts

Ordered before the editor because every later phase consumes this layer.

- `KeyCombo` parsing and formatting.
- TOML binding loading, scope resolution, conflict reporting.
- Keyboard capability probe, Kitty enhancement flags, `safe` preset.
- `InputMode` routing per 5.3.
- `PromptState` with the command palette as the first consumer.

Definition of done: parse and conflict tests pass; typing in a prompt never mutates a document; Esc closes the palette; a conflicting binding produces a visible warning.

### Phase 3 — File Explorer

- Directory tree with lazy per-directory loading.
- Expand/collapse directories.
- Keyboard navigation.
- File selection.
- Open file.
- Basic filesystem operations, each destructive one behind a confirmation prompt.
- Hidden file toggle and `.gitignore` respect.

Definition of done: startup does not walk the whole tree; a directory with 50,000 entries does not freeze input; create, rename, and delete work with confirmation.

### Phase 4 — Editor Engine

- Ropey integration.
- Document model with stable `DocumentId`.
- Grapheme-aware cursor movement.
- Text insertion and deletion.
- Scrolling with saturating viewport arithmetic.
- Line numbers and display-width rendering.
- File saving with line-ending preservation.
- Modified state.
- Undo and redo per 5.3.1.

Definition of done: Unicode width, grapheme, and undo-coalescing tests pass; editing a large file does not clone the rope per keystroke.

### Phase 5 — Document Tabs

- Multiple open documents.
- Document tab rendering inside a pane.
- Switch documents.
- Close documents.
- Unsaved changes handling.
- Document registry.

Definition of done: multiple document tabs per pane; closing a dirty document always prompts; a document shared by two panes shows one buffer.

### Phase 6 — Split Layout Engine

- Weighted multi-child splits along `LeftRight` and `TopBottom`.
- Vertical split (left and right, `LeftRight`).
- Horizontal split (top and bottom, `TopBottom`).
- Nested split.
- Pane focus.
- Pane navigation via `Ctrl+Shift+Arrow`.
- Separator resizing with minimum-size clamping.
- Close pane, with tab reassignment.
- Redundant split collapse.

Definition of done: property test over random trees and terminal sizes produces no empty text area and no panic; collapse invariants hold; `PaneId` references stay valid.

### Phase 7 — Workspace Management

- Create workspace.
- Switch workspace.
- Close workspace.
- Workspace tab bar.
- Preserve workspace layouts.
- Preserve document tabs and per-pane cursor and scroll.

Definition of done: switching workspaces restores layout, tabs, cursor, and scroll; closing a workspace never drops a dirty document silently.

### Phase 8 — MVP Stabilization

- Configuration loading and validation.
- Keyboard mapping presets.
- Unicode handling review.
- Terminal resize robustness.
- Error handling and status messages.
- Integration tests, including `TestBackend` render snapshots.
- Documentation.
- README with installation and usage instructions.

Deliverable: a usable MVP.

## 11. Testing Requirements

Every important module must have unit tests.

Prioritize:

- Document buffer operations.
- Cursor movement.
- Unicode positioning, including CJK width and combining characters.
- Grapheme-level deletion and cursor motion.
- Undo and redo: apply N random edits, undo N times, assert the buffer equals the original.
- Undo coalescing: typing bursts form one batch, cursor moves break it.
- Keymap resolution: every default binding parses and resolves to its action.
- Keymap conflicts: duplicate bindings are reported, not silently resolved.
- Input routing: a prompt consumes keys, a document does not.
- Layout tree construction.
- Minimum-size clamping: random trees against random terminal sizes.
- Split and close operations.
- Split resizing, including clamping at minimum size.
- Collapse invariants: no single-child split, no empty split, no orphaned `PaneId`.
- Workspace switching.
- Document switching.
- Unsaved changes handling, including closing a workspace with dirty documents.

Rendering is part of the contract. Use Ratatui's `TestBackend` to snapshot at least: the initial layout, a nested split at 80×24, a split at the minimum size, and a prompt overlay. Logic-only unit tests do not catch layout regressions.

Run:

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features
```

Fix relevant errors and warnings before considering a milestone complete.

Do not claim a test passed unless it was actually executed.

## 12. Engineering Rules

Follow these rules throughout development:

1. Write idiomatic Rust.
2. Prefer clear ownership and borrowing.
3. Avoid unnecessary cloning of large document buffers.
4. Use stable IDs for documents, panes, and workspaces.
5. Avoid circular module dependencies.
6. Keep UI rendering separate from business logic.
7. Keep filesystem operations separate from text editing.
8. Use meaningful error types.
9. Avoid unnecessary abstractions.
10. Write tests for non-trivial logic.
11. Document complex architectural decisions.
12. Never silently discard unsaved changes.
13. Ensure terminal cleanup even when errors occur.
14. Keep dependencies minimal.
15. Do not implement future-phase features prematurely.

## 13. Coding Agent Execution Instructions

You must work directly in the current repository.

Follow this execution order:

### Step 1 — Inspect

- Inspect the current repository.
- Determine whether it is empty or contains existing code.
- Preserve useful existing files.
- Do not overwrite unrelated user work.

### Step 2 — Establish Documentation

Create or update:

- AGENTS.md
- SPEC.md
- ARCHITECTURE.md
- DATA-MODEL.md
- ROADMAP.md

These documents must reflect this specification.

### Step 3 — Bootstrap

- Initialize the Rust project if necessary.
- Configure Cargo dependencies.
- Establish the modular folder structure.
- Create the initial application entry point.

### Step 4 — Implement

Start with Phase 1 and proceed sequentially.

Do not implement all phases in one giant code generation operation.

After completing each phase:

1. Run formatting.
2. Run cargo check.
3. Run relevant tests.
4. Fix errors.
5. Update ROADMAP.md.
6. Summarize completed work.
7. Continue to the next phase if no blocker exists.

Do not stop after creating documentation or project scaffolding.

Continue implementing the project until all planned MVP phases are completed, unless a genuine blocker requires user input.

If a decision is not specified, choose the simplest maintainable solution consistent with this architecture. Document significant assumptions.

Do not ask for approval for ordinary implementation decisions.

## 14. Final Deliverables

At completion, provide:

1. Working Rust source code.
2. Complete Cargo.toml.
3. Project documentation.
4. Configuration example.
5. Unit and integration tests.
6. README with installation instructions.
7. Keyboard shortcut reference.
8. Build and execution commands.
9. Summary of implemented features.
10. Known limitations and suggested next steps.

The final application must be executable using:

```bash
cargo run
```

It must also support opening a directory or file from the command line.

Example:

```bash
cargo run -- .
cargo run -- ./src/main.rs
```

---

## FINAL INSTRUCTION

Begin by inspecting the repository and establishing the project structure.

Then implement i3-editor phase by phase.

Prioritize a working, testable application over excessive abstraction or premature advanced features.

Do not return only an implementation plan. Write the actual code, execute the tests, and maintain the project documentation as you work.
