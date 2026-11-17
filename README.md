# i3-editor

A keyboard-first terminal text editor written in Rust: file explorer, document
tabs, panes, and workspaces, all inside the terminal.

Built with Ratatui + Crossterm, with a Ropey text buffer and a grapheme-aware
cursor. Configuration lives in TOML.

## Build and run

```bash
cargo build --release
cargo run                      # open the current directory
cargo run -- .                 # same, explicit
cargo run -- src/main.rs       # open a file directly
cargo run -- --dump-keymap     # print the resolved bindings and exit
```

Stable Rust 1.85+ (edition 2024). No system dependencies beyond a POSIX
terminal.

## Interface

```text
 workspace tabs (F7 new, F8 close)
+--------------------------------------+
| Explorer   | pane 1        | pane 2  |
|            | tabs / text   |         |
+--------------------------------------+
 status bar: file | line:col | lines | UTF-8 EOL | focus | message
```

- Explorer: 20% of the width by default, collapsible with `Ctrl+B`.
- Panes: recursive weighted splits, independent cursor, selection, and scroll.
- Workspaces: independent layouts and tabs; switching preserves everything.

## Default keys

Terminals cannot represent every modifier combination, so the defaults avoid
the undecidable ones: no `Ctrl+Shift+<letter>`, no `Ctrl+H` (it is Backspace),
no `Alt+<letter>`, no `Ctrl+2`..`Ctrl+8`.

| Key | Action |
|---|---|
| `Ctrl+Q` | Quit |
| `Ctrl+S` | Save |
| `F2` | Save As |
| `Ctrl+O` / `Ctrl+P` | Quick open |
| `Ctrl+N` | New buffer |
| `Ctrl+W` | Close document (prompts when dirty) |
| `Ctrl+B` | Toggle explorer / focus explorer |
| `F1` | Command palette |
| `Ctrl+G` | Go to line |
| `Ctrl+Tab` / `BackTab` | Next / previous document |
| `F3` / `F4` | Split left-right / top-bottom |
| `F6` | Close pane |
| `Ctrl+Shift+Arrow` | Move focus between panes |
| `F9` | Toggle fullscreen for the focused pane |
| `F10` | Grab mode: arrows move the focused pane (Esc exits) |
| `F12` | Toggle tabbed container for the focused split |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next container tab |
| `F7` / `F8` | New / close workspace |
| `Ctrl+1`..`Ctrl+9` | Switch workspace (best effort) |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Shift+Arrow`, `Shift+Home/End` | Extend selection |
| `Ctrl+A` | Select all |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste |
| `Ctrl+F` / `Ctrl+R` | Find / replace all |
| `Ctrl+D` | Duplicate line |

Explorer focus keys: `Up`/`Down`, `Enter` (open), `Right`/`Left` (expand or
collapse), `n` new file, `Shift+N` new directory, `r` rename, `d` delete,
`h` hidden files, `Ctrl+R` refresh.

Mouse: click a pane to focus it, drag a pane border to resize the split.

- Editor: click places the cursor at that cell (tabs and wide characters are
  measured in display cells); `Shift`+click extends the selection; the wheel
  scrolls three lines without moving the cursor; `ScrollLeft`/`ScrollRight`
  (horizontal wheel or `Shift`+wheel) scrolls four columns.
- Explorer: click selects a row, double-click opens the file (or toggles a
  directory), wheel moves the selection three rows at a time.
- Container tab strip: click a tab to activate it.

### Open two files side by side

1. Open the first file (`Ctrl+O` or `Ctrl+P`).
2. `F3` (left/right) or `F4` (top/bottom) splits the focused pane; the new pane
   inherits the same file.
3. Open the second file while that pane is the last one edited — click it, or
   press `Ctrl+B` and pick the file in the explorer. It opens in the remembered
   pane, not in the first pane.
4. `F10` then an arrow swaps the focused pane with its neighbour; `Esc` leaves
   grab mode.

Each pane keeps its own tabs, cursor, selection, and scroll, even when they show
the same buffer.

### Tabbed container (i3 `layout tabbed`)

`F12` converts the innermost split around the focused pane into a tabbed
container: siblings stop tiling, the container keeps a one-row tab strip, and
only the active tab is laid out. `F12` again returns to tiled layout with the
split weights intact.

- `Ctrl+PageDown` / `Ctrl+PageUp` cycle tabs; clicking a tab does the same.
- Splitting inside a tabbed container opens the new pane as a new tab.
- Closing the last tab of a container collapses it into the remaining child.
- `F9` fullscreen hides the strip; `F10` grab mode still swaps the pane.
- Tabs are per container, so nested tabbed containers each get their own strip.

## Configuration

`$XDG_CONFIG_HOME/i3-editor/config.toml` or `~/.config/i3-editor/config.toml`.
Every section and key is optional; a broken section is reported and replaced by
its default instead of blocking startup.

```toml
[editor]
tab_size = 4
insert_spaces = true
line_numbers = true
word_wrap = false          # accepted, not applied yet
auto_indent = true

[explorer]
visible = true
width = 20                 # percent, clamped to 5..=50
show_hidden = false

[workspace]
default_layout = "vertical"
auto_save_session = false  # accepted, not applied yet

[appearance]
theme = "default"          # accepted, not applied yet
show_status_bar = true
show_document_tabs = true
show_workspace_tabs = true

[keys]
preset = "default"         # "default" | "safe"

[keys.global]
quit = "ctrl+q"
save_as = "f2"
split_left_right = "f3"

[keys.editing]
find = "ctrl+f"
replace = "ctrl+r"

[keys.explorer]
rename = "r"

```

Binding values are `ctrl+`/`alt+`/`shift+` plus a key name (`a`, `f2`, `enter`,
`tab`, `backtab`, `esc`, `space`, `home`, `end`, `pageup`, `pagedown`, `delete`,
`backspace`, arrows). Actions are the names printed by `--dump-keymap`.
Conflicts and unknown names are reported, never silently dropped.

## Verify

```bash
cargo fmt --check
cargo check
cargo test              # 56 unit + integration tests, no terminal required
cargo clippy --all-targets --all-features
```

## Known limitations

- `word_wrap`, `theme`, and `auto_save_session` are parsed but not applied.
- Syntax highlighting (Tree-sitter), fuzzy scoring (Nucleo), and filesystem
  watching (Notify) are not implemented.
- Undo restores text, cursor, and scroll, but a document that has been saved
  then undone still reports as modified.
- Clipboard writes are best effort: the internal clipboard always works, OSC 52
  depends on the terminal.
- Startup probes keyboard enhancement support; terminals that never answer the
  query add a short delay to startup.
- Session persistence across restarts is not implemented.
