# Regenerate docs/demo.gif with Charm VHS (https://github.com/charmbracelet/vhs).
#
# Prepare the throwaway project first (the tape types keys, not setup commands,
# because VHS' Type parser cannot carry embedded double quotes):
#
#   rm -rf /tmp/i3-demo && mkdir -p /tmp/i3-demo/src
#   printf 'fn main() {\n    println!("hello");\n}\n' > /tmp/i3-demo/src/main.rs
#   printf '# Demo\n\n- panes\n- git\n' > /tmp/i3-demo/README.md
#   cd /tmp/i3-demo && git init -q && git add -A \
#     && git -c user.email=demo@example.com -c user.name=demo commit -qm base
#   printf 'fn main() {\n    println!("hello, world");\n}\n' > src/main.rs
#
# Then, from the repository root with `i3-editor` on PATH:
#
#   vhs docs/demo.tape
#
# Bindings follow README.md: Ctrl+P quick open, Ctrl+Space completion,
# Ctrl+S save, Ctrl+Q quit. Function keys (F1 palette, F3 split) are left out
# because this VHS release rejects them.

Output docs/demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 904
Set Height 600
Set Padding 0
Set Theme "Dracula"
Set TypingSpeed 60ms

Type "cd /tmp/i3-demo"
Enter
Type "i3-editor src/main.rs"
Enter
Sleep 1.2s

# Quick open the second file, then come back to the Rust buffer.
Ctrl+P
Type "README"
Sleep 400ms
Enter
Sleep 900ms
Ctrl+P
Type "main.rs"
Sleep 400ms
Enter
Sleep 700ms

# Type, then ask the language server for a completion.
Type "    let greeting = 'h';"
Sleep 500ms
Enter
Type "    pri"
Sleep 300ms
Ctrl+Space
Sleep 1.6s
Escape
Sleep 300ms

# Save and leave.
Ctrl+S
Sleep 500ms
Ctrl+Q
Sleep 900ms
Enter
Sleep 600ms
