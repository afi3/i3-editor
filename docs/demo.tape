# Regenerate docs/demo.gif with Charm VHS (https://github.com/charmbracelet/vhs).
#
# Prepare the throwaway project and demo config first (the tape types keys, not
# setup commands, because VHS' Type parser cannot carry embedded double quotes):
#
#   rm -rf /tmp/i3-demo && mkdir -p /tmp/i3-demo/src
#   printf 'fn main() {\n    println!("hello");\n}\n' > /tmp/i3-demo/src/main.rs
#   printf '# Demo\n\n- panes\n- git\n' > /tmp/i3-demo/README.md
#   cd /tmp/i3-demo && git init -q && git add -A \
#     && git -c user.email=demo@example.com -c user.name=demo commit -qm base
#   printf 'fn main() {\n    println!("hello, world");\n}\n' > src/main.rs
#   mkdir -p /tmp/i3-demo-config/i3-editor
#   cp docs/demo-config.toml /tmp/i3-demo-config/i3-editor/config.toml
#
# Re-run the fixture setup before every recording: the demo saves at the end, so
# a second run would start from the edited file.
#
# Then, from the repository root with `i3-editor` on PATH:
#
#   vhs docs/demo.tape
#
# Bindings: Ctrl+E palette and Ctrl+B split come from docs/demo-config.toml,
# because this VHS release rejects F1/F3. Ctrl+P quick open, Ctrl+S save and
# Ctrl+Q quit are the shipped defaults.

Output docs/demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 904
Set Height 600
Set Padding 0
Set Theme "Dracula"
Set TypingSpeed 45ms

Hide
Type "cd /tmp/i3-demo"
Enter
Type "export XDG_CONFIG_HOME=/tmp/i3-demo-config"
Enter
Type "i3-editor src/main.rs"
Enter
Sleep 1.4s
Show

# Split the pane, then use the palette for the Git diff overlay.
Ctrl+B
Sleep 900ms
Ctrl+E
Type "git diff"
Sleep 600ms
Enter
Sleep 1.6s
Escape
Escape
Sleep 500ms

# Quick open the second file, then come back to the Rust buffer.
Ctrl+P
Type "README"
Sleep 500ms
Enter
Sleep 1s
Ctrl+P
Type "main.rs"
Sleep 500ms
Enter
Sleep 900ms

# Type a line, then save and leave.
Type "    let total = 42;"
Sleep 700ms
Enter
Type "    let done = true;"
Sleep 800ms
Escape
Sleep 300ms
Ctrl+S
Sleep 600ms
Ctrl+Q
Sleep 900ms
Enter
Sleep 600ms
