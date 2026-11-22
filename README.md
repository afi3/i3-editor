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
  | sh -s -- --version 0.1.1

# update an existing install (installer or the binary itself)
i3-editor --update
curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh \
  | sh -s -- --update

# remove the binary (settings and themes are kept)
curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh \
  | sh -s -- --uninstall
```

`--update` asks the GitHub API for the latest release and compares it with
`i3-editor --version`; combine the installer form with `--version` to pin.
`--uninstall` removes only the binary and reports the configuration directory,
which stays on disk unless you delete it.

### Self-update

`i3-editor --update` performs the same update without the installer. Releases
before 0.1.1 predate the flag, so update those once with the installer form above:

1. Reads `https://api.github.com/repos/afi3/i3-editor/releases/latest`.
2. Exits with `already up to date` when the running version is current, and does
   not downgrade when the running build is newer.
3. Downloads `i3-editor-<target>.tar.gz` and its `.sha256` from the release,
   verifies the checksum, and extracts the binary.
4. Replaces the canonical executable through a sibling temporary file and a
   rename, so a failed update never truncates the running binary.

It needs `curl` and `tar` (both present on macOS and Linux by default), write
permission to the binary's directory, and network access. Configuration, themes,
and the session file are untouched. If permission is denied, rerun with `sudo`
or use the installer with `--prefix`. The checksum comes from the same release,
so it protects against transfer corruption but not a compromised repository;
there is no signature. For mirrors or local testing, `I3_EDITOR_UPDATE_API`
(full API URL) and `I3_EDITOR_UPDATE_BASE` (release download base, the tag is
appended) override the endpoints.

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
cargo run -- --update          # self-update from the latest release
cargo run -- --dump-keymap     # print the resolved bindings and exit
cargo run -- --version         # print the version
cargo run -- --help            # usage summary
```

Stable Rust 1.85+ (edition 2024). The editor needs no system dependencies beyond
a POSIX terminal; `--update` additionally uses `curl` and `tar`.

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

Startup probes keyboard enhancement support. Without it the `safe` preset is
forced and the status bar names the actions whose default binding needs
enhancement (`switch_workspace`); `next_workspace` / `prev_workspace` in the
palette keep every workspace reachable. `Ctrl+C`
closes an active prompt; with no selection it reports `nothing to copy`.
Bracketed paste is routed by owner: a prompt field while a prompt is open, the
PTY while the terminal panel has focus, and the active document otherwise. The
prompt overlay also captures mouse input, so clicks cannot move the cursor behind
it.

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
| `F5` | New terminal tab |
| `F11` | Close the active terminal tab |
| `F6` | Close pane |
| `Ctrl+Shift+Arrow` | Move focus between panes |
| `F9` | Toggle fullscreen for the focused pane |
| `F10` | Grab mode: arrows move the focused pane (Esc exits) |
| `F12` | Toggle tabbed container for the focused split |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next container tab, or terminal tab while the shell has focus |
| `F7` / `F8` | New / close workspace |
| `Ctrl+1`..`Ctrl+9` | Switch workspace (best effort) |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Shift+Arrow`, `Shift+Home/End` | Extend selection |
| `Ctrl+A` | Select all |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste |
| `Ctrl+F` / `Ctrl+R` | Find / replace all |
| `Ctrl+D` | Duplicate line |
| `Tab` | Indent: spaces to the next stop, or a literal tab |

Explorer focus keys: `Up`/`Down`, `Enter` (open), `Right`/`Left` (expand or
collapse), `n` new file, `Shift+N` new directory, `r` rename, `d` delete,
`h` hidden and ignored files (such as `.env`), `Ctrl+R` refresh.

Mouse: click a pane to focus it, drag a pane border to resize the split.

- Editor: click places the cursor at that cell (tabs and wide characters are
  measured in display cells); `Shift`+click extends the selection; the wheel
  scrolls three lines without moving the cursor; `ScrollLeft`/`ScrollRight`
  (horizontal wheel or `Shift`+wheel) scrolls four columns.
- Explorer: click selects a row, double-click opens the file (or toggles a
  directory), wheel moves the selection three rows at a time.
- Container tab strip: click a tab to activate it.

### Quick open and command palette

`Ctrl+P` / `Ctrl+O` list project files and `F1` lists commands. Both match fuzzy,
scored by [Nucleo](https://github.com/helix-editor/nucleo): the query is a
subsequence of the candidate, with bonuses at path separators, underscores, and
word boundaries, and smart case (an uppercase letter in the query makes matching
case-sensitive). The file list is scanned once and kept current by the
filesystem watcher; quick open no longer walks the project on every invocation.

| Query | Result |
|---|---|
| `sm` | `src/main.rs`, ranked above `docs/domain_main.md` |
| `tt` | `toggle_terminal` |
| `SM` | only candidates containing uppercase `S` then `M` |

Enter opens the selected candidate. While typing, the best-ranked candidate is
selected, so Enter acts on what the list shows. After `Up` / `Down`, the chosen
candidate is kept across further keystrokes; it falls back to the best match once
it no longer matches the query.

### Find and replace

`Ctrl+F` opens Find and searches as you type: every match is highlighted in the
document, the prompt lists matching lines with their line numbers (`Find 2/57` in
the title), and the cursor jumps to the selected match.

- `Up` / `Down` step through the list and wrap at both ends.
- `Enter` keeps the selected match, closes the prompt, and leaves the search
  active; `next_match` from the palette (`F1`) continues from there.
- The list shows up to eight rows around the selection; the buffer may hold more.
- `Ctrl+R` replaces every occurrence of the first field with the second.

### Find in files

Find in Files searches every project file listed by quick open and streams the
matches into the prompt as they arrive. There is no default key because terminals
cannot distinguish `Ctrl+Shift+F`; run `find_in_files` from the command palette
(`F1`), or bind it in `config.toml` if your terminal reports the combination:

```toml
[keys.editing]
"ctrl+shift+f" = "find_in_files"
```

- Matching is literal and smart-case: an all-lowercase query ignores case, any
  uppercase character makes it case-sensitive.
- Results show `path:line:column` plus the matching line. `Up` / `Down` choose a
  result; `Enter` opens that file and puts the cursor on the match; `Esc` cancels
  the scan and closes the prompt.
- Typing restarts the scan. The previous worker is cancelled, so only the newest
  query can publish results.
- The scan searches files on disk, not unsaved buffer contents, skips files above
  1 MiB and files that are not UTF-8, and stops at 1000 matches (the title shows
  the count with a `+`).
- Files hidden by `.gitignore` are excluded unless the explorer's hidden toggle
  is on, matching quick open.

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

Rust, Python, JavaScript/TypeScript, Go, C, Shell, TOML, JSON, and Markdown are
highlighted from tree-sitter parse trees. The grammar's highlight query decides
the token kind, so Rust raw strings, shell heredocs, nested block comments, and
template literals keep their color across line breaks, and identifiers are
classified by their role in the tree rather than by naming convention. Each
document keeps its tree and hands every edit to it as an `InputEdit`; the next
frame reparses only the changed region and runs the query over the visible lines
instead of the whole file.

Markdown uses five grammars: the block grammar highlights headings, fences, and
links; visible inline content is parsed with the inline grammar for emphasis,
links, and code spans; HTML blocks plus inline tags use the HTML grammar; YAML
front matter (`--- ... ---`) uses the YAML grammar and TOML front matter
(`+++ ... +++`) the existing TOML grammar. Each fenced block is
parsed with the language named in its info string, so Rust, Python, Shell, TOML,
JSON, and the other supported languages are highlighted structurally inside the
fence. These secondary trees are cached by byte offset and dropped on edit.

Files with other extensions render in the theme foreground.

Known limit: an unknown fence language stays plain text.

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
- A save writes a sibling temporary file, flushes it, and renames it over the
  target: a crash or full disk during the write cannot truncate the original.
  Existing permissions are preserved and symlinks are resolved, so the link stays.
- `F2` Save As asks for confirmation before replacing a different existing file.
- The workspace root is watched recursively. A clean buffer reloads when
  another program writes its file; a modified buffer is left alone and the status
  bar reports the change, so `Ctrl+S` still asks before anything is written.
  Explorer rows refresh on create, rename, and delete without pressing refresh.
- If another program changed the file since it was loaded, `Ctrl+S` asks first:
  `Overwrite with the buffer` or `Reload from disk`. Nothing is written until you
  choose, so an external edit is never clobbered silently.

### Sessions

On exit the editor writes `<config>/i3-editor/session.toml`: workspaces, layout
trees, pane tabs (saved files only), cursor, and scroll. Launching without a path
argument in the same directory restores it; launching with a path starts fresh.
Untitled buffers are not persisted, files that no longer exist are skipped with a
status-bar warning, and closing every document removes the session file. The
session is validated (version, counts, layout invariants) before it can replace
the default workspace; a broken session is reported and ignored.

## Terminal panel

The editor can host real shells in a panel at the bottom of the window, outside
the panes. Open it from the command palette (`F1`, then `toggle_terminal`); the
same command closes the panel and kills every shell in it.

- `F5` (or `new_terminal`) adds a shell tab and focuses it; `F11` (or
  `close_terminal`) closes the active tab and closes the panel with the last one.
  `Ctrl+PageUp` / `Ctrl+PageDown` cycle tabs while the panel has focus; the title
  marks the active one as `[2* /bin/zsh]`.
- The shell comes from `[terminal] shell`, otherwise `$SHELL`, otherwise
  `/bin/sh`; it starts in the explorer root.
- While the panel has focus, every key is forwarded to the shell except `Ctrl+Q`,
  `F1`, the terminal toggle, and the terminal-tab commands.
- Click the panel to focus it. When the program in the shell enables mouse
  reporting (`DECSET 1000`, `1002`, or `1003`), clicks, drags, and wheel events
  inside the panel are sent to it using the encoding it asked for (SGR, UTF-8,
  or X10); the protocol mode gates presses, releases, and motion. Otherwise the
  wheel scrolls `[terminal] scrollback` lines (default 2000).
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

The palette also lists every explorer action, `messages`, and
`switch_workspace_N` for each open workspace; `next_workspace` / `prev_workspace`
cycle through them. `messages` opens the full startup warning list when the
status bar only has room for the first one plus a count.

## Configuration

`$XDG_CONFIG_HOME/i3-editor/config.toml` or `~/.config/i3-editor/config.toml`.
Every section and key is optional; a broken section is reported and replaced by
its default instead of blocking startup. Unknown sections and unknown keys are
reported too, and an unknown key keeps the rest of its section intact, so a typo
never looks like a working key and never discards valid settings.

```toml
[editor]
tab_size = 4
insert_spaces = true
line_numbers = true
auto_indent = true

[explorer]
visible = true
width = 20                 # percent, clamped to 5..=50
show_hidden = false

[appearance]
theme = "default"          # preset, or a themes/<name>.toml file name
show_status_bar = true
show_document_tabs = true
show_workspace_tabs = true

[terminal]
height = 12                # rows for the bottom panel, clamped to 3..=60
shell = "/bin/zsh"         # optional: default $SHELL, then /bin/sh
scrollback = 2000          # lines kept, clamped to 100..=100000

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
Conflicts, unknown names, and invalid values are reported with the config path
and line, never silently dropped: a binding may not take a combination reserved
for quit. The status bar shows the first warning and how many remain; `messages`
in the palette lists all of them.
`I3_EDITOR_DEBUG=1` writes the resolved table with the enhancement flag to
`<config>/i3-editor/keymap.log` at startup, next to the config file.

`insert_spaces = true` makes `Tab` insert spaces up to the next `tab_size` stop
(measured in display cells, so it aligns after existing tabs and wide characters);
`insert_spaces = false` inserts a literal tab byte.

## Verify

```bash
cargo fmt --check
cargo check
cargo test              # 228 unit + integration tests, no terminal required
cargo clippy --all-targets --all-features
python3 scripts/make_demo.py   # regenerate docs/demo.gif (needs pyte + Pillow)
```

## Releasing

Users receive a new version only after a `v*` tag publishes a release; pushing
commits to `main` changes neither the latest release nor `--update`.

```bash
# 1. bump the version in Cargo.toml (Cargo.lock follows)
cargo test
cargo clippy --all-targets --all-features

# 2. commit, then tag the same commit
git tag -a v0.1.1 -m "v0.1.1"
git push origin main
git push origin v0.1.1

# 3. the release workflow builds four targets and publishes assets:
#    i3-editor-<target>.tar.gz + .sha256
gh release view v0.1.1

# 4. verify the updater against the published release
i3-editor --update
```

The version in `Cargo.toml` must match the tag. `--update` compares versions, so
a tag whose binary still reports the old version would be downloaded again on
every check.

## Known limitations

- Syntax highlighting is structural for every supported language. An unknown
  fence language stays plain text.
- Only the workspace root is watched. A document saved outside it is not
  reloaded automatically, and quick open still caps its cached list at 5000
  files.
- `--update` uses the unauthenticated GitHub API (60 requests per hour per IP)
  and trusts the checksum published in the same release; there is no signature.
- Saving replaces the file by rename, so an extra hard link keeps pointing at the
  old inode. Symlinked files are updated through the link.
- Clipboard writes are best effort: the internal clipboard always works, OSC 52
  depends on the terminal.
- Startup probes keyboard enhancement support; terminals that never answer the
  query add a short delay to startup.
