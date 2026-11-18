# i3-editor

A keyboard-first terminal text editor written in Rust: file explorer, document
tabs, panes, and workspaces, all inside the terminal.

Built with Ratatui + Crossterm, with a Ropey text buffer and a grapheme-aware
cursor. Configuration lives in TOML.

![i3-editor demo](docs/demo.gif)

## Install

macOS (Intel and Apple Silicon) and Linux (x86_64 and aarch64):

```bash
curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh | sh
```

The installer downloads the release asset for your platform, verifies its
SHA-256 checksum, and puts `i3-editor` in `~/.local/bin` (add that to `PATH` if the
script asks). Options:

```bash
# custom location
curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh \
  | sh -s -- --prefix "$HOME/bin"

# pinned version
curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh \
  | sh -s -- --version 0.1.0
```

`--help` prints all options; `I3_EDITOR_INSTALL_DIR` and `I3_EDITOR_VERSION` work
too. If no matching release asset exists yet, the script falls back to
`cargo install --git https://github.com/afi3/i3-editor --locked`, which needs a
Rust toolchain from <https://rustup.rs>.

Release binaries are produced by `.github/workflows/release.yml` when a `v*` tag
is pushed. Without Cargo and without a release, install Rust first or build from
source as shown below.

## Build and run

```bash
cargo build --release
cargo run                      # open the current directory
cargo run -- .                 # same, explicit
cargo run -- src/main.rs       # open a file directly
cargo run -- --dump-keymap     # print the resolved bindings and exit
cargo run -- --version         # print the version
cargo run -- --help            # usage summary
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

## Themes and syntax colors

`appearance.theme` names either a built-in preset or a file in
`$XDG_CONFIG_HOME/i3-editor/themes/` (default `~/.config/i3-editor/themes/`).
A theme file wins over a preset with the same name, so `dark.toml` overrides the
`dark` preset. Unknown names fall back to `default` and report the available
names in the status bar.

| Preset | Look |
|---|---|
| `default` | Terminal colors; no background override, blue selection |
| `dark` | Dark background, muted gutter, cyan accent, VS Code-like tokens |
| `light` | Light background, dark text, classic light token colors |
| `high-contrast` | Black on white with yellow cursor line, border, and selection |

### Theme files

Drop `<name>.toml` in the themes directory and set `appearance.theme = "<name>"`.
Every key is optional; `base` inherits from a preset, so a file can override just
a few colors. Unknown keys are reported instead of being ignored.

```toml
# ~/.config/i3-editor/themes/mine.toml
base = "dark"                 # preset to inherit from (default: "default")

background = "#101014"
foreground = "#e6e6e6"
current_line = "#18181f"
selection = "#2b4b8f"
selection_text = "#ffffff"
gutter = "#5a5a66"
gutter_active = "#e6e6e6"
border_active = "#7aa2f7"
border_inactive = "#2a2a33"
tab_active = "#1c1c24"
tab_inactive = "#5a5a66"
status_bar = "#1c1c24"
status_text = "#e6e6e6"
explorer_dir = "#7aa2f7"
prompt_border = "#7aa2f7"

[syntax]
keyword = "#c678dd"
string = "#98c379"
comment = "#5c6370"
number = "#d19a66"
function = "#61afef"
type_name = "#e5c07b"
constant = "#56b6c2"
operator = "#abb2bf"
punctuation = "#8b93a1"
```

Colors accept `#rgb`, `#rrggbb`, `indexed:0..255`, or names such as `reset`,
`black`, `red`, `…`, `white`, `gray`, `darkgray`.

### Syntax highlighting

Highlighting is lexical and line-local: comments, strings, numbers, keywords,
function calls, type names, constants, operators, and punctuation are colored per
line for Rust, Python, JavaScript/TypeScript, Go, C, Shell, TOML, JSON, and
Markdown. Files with other extensions render in the theme foreground.

Known limits of the lexer: blocks that span lines (Rust `/* */`, Python triple
quotes, JS template literals) are highlighted line by line rather than as a whole
block, and identifiers are classified by naming convention (`Thing` → type,
`THING` → constant, `name(` → function). Structural, grammar-aware highlighting
is still the Tree-sitter work listed in `ROADMAP.md`.

The terminal backend honors `NO_COLOR`: when that variable is set, color
sequences are suppressed and the theme has no visible effect. `--dump-keymap`
prints the resolved theme name, which is the quickest way to confirm a config and
theme file were read.

### Saving and quitting

- `Ctrl+Q` with modified documents opens a prompt with `Save all and quit` and
  `Discard all and quit`; arrow keys choose, `Enter` confirms, `Esc` cancels.
  Untitled buffers must be saved first (they only offer discard).
- `Ctrl+W` on a modified document asks before discarding.
- `F6` is refused while the pane still holds modified documents; save or close
  them first, so no document becomes unreachable.
- Undo keeps the last 1000 batches. Undoing back to the saved revision clears the
  modified marker, and redo restores it.

## Terminal panel

The editor can host a real shell in a panel at the bottom of the window, outside
the panes. Open it from the command palette (`F1`, then `toggle_terminal`); the
same command closes it, which kills the shell process.

- The shell is `$SHELL` (fallback `/bin/sh`), started in the explorer root.
- While the panel has focus, every key is forwarded to the shell except `Ctrl+Q`,
  `F1`, and the terminal toggle itself.
- Click the panel to focus it; the wheel scrolls 2000 lines of scrollback.
- The panel height comes from `[terminal] height`; panes keep their minimum size,
  so the panel shrinks instead of squeezing them out.
- Output is parsed with `vt100` (colors, bold, underline, inverse, cursor) and
  rendered with the active theme's background.

Frame redraws run at 30 ms while the shell has focus, 60 ms when the panel is
visible but unfocused, and 250 ms otherwise.

## Switching themes from the palette

`F1` exposes three theme commands:

| Action | Effect |
|---|---|
| `select_theme` | Prompt listing every preset and `themes/*.toml` file |
| `next_theme` | Apply the next theme in that list |
| `prev_theme` | Apply the previous theme |

The switch is immediate and updates `appearance.theme` in memory **and in the
config file**: the `theme` line is rewritten while other sections, comments, and
key bindings stay untouched. If the write fails, the theme still applies and the
status bar reports the error. Presets, syntax colors, status bar, and explorer all
follow without a restart.

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
theme = "default"          # preset, or a themes/<name>.toml file name
show_status_bar = true
show_document_tabs = true
show_workspace_tabs = true

[terminal]
height = 12                # rows for the bottom panel, clamped to 3..=60

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
`backspace`, arrows). `--dump-keymap` prints bound actions; the palette (`F1`)
also lists unbound actions such as `toggle_terminal` and the theme commands.
Conflicts and unknown names are reported, never silently dropped.

## Verify

```bash
cargo fmt --check
cargo check
cargo test              # 133 unit + integration tests, no terminal required
cargo clippy --all-targets --all-features
```

## Known limitations

- `word_wrap` and `auto_save_session` are parsed but not applied.
- Syntax highlighting is lexical, not grammar-aware: multiline block comments and
  template strings are colored line by line. Tree-sitter remains future work.
- Fuzzy scoring (Nucleo) and filesystem watching (Notify) are not implemented.
- Undo restores text, cursor, and scroll, but a document that has been saved
  then undone still reports as modified.
- Clipboard writes are best effort: the internal clipboard always works, OSC 52
  depends on the terminal.
- Startup probes keyboard enhancement support; terminals that never answer the
  query add a short delay to startup.
- Session persistence across restarts is not implemented.
- Terminal panel: one shell only, no mouse reporting into shell programs, shell
  path is `$SHELL`, scrollback fixed at 2000 lines.
