# Regenerate docs/demo.gif with Charm VHS (https://github.com/charmbracelet/vhs).
#
#   brew install vhs
#   vhs docs/demo.tape
#
# VHS is not installed in this environment, so the recording itself has not been
# produced or verified here: the script is the reproducible source for it.
# Bindings below match README.md: F3 splits left/right, F1 is the palette,
# Ctrl+P is quick open, Ctrl+Space is completion, Ctrl+S saves, Ctrl+Q quits.

Output docs/demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 904
Set Height 600
Set Padding 0
Set Theme "Dracula"
Set TypingSpeed 60ms

# A throwaway project: one tracked file with a local edit, one untracked file.
Hide
Type "rm -rf /tmp/i3-demo && mkdir -p /tmp/i3-demo/src"
Enter
Type "printf 'fn main() {\n    println!(\"hello\");\n}\n' > /tmp/i3-demo/src/main.rs"
Enter
Type "printf '# Demo\n\n- panes\n- git\n' > /tmp/i3-demo/README.md"
Enter
Type "cd /tmp/i3-demo && git init -q && git add -A && git -c user.email=demo@example.com -c user.name=demo commit -qm base"
Enter
Type "printf 'fn main() {\n    println!(\"hello, world\");\n}\n' > src/main.rs"
Enter
Show

Type "i3-editor src/main.rs"
Enter
Sleep 1.2s

# Quick open the second file, then split the pane and come back.
Ctrl+p
Type "README"
Sleep 400ms
Enter
Sleep 800ms
F3
Sleep 900ms
Ctrl+p
Type "main.rs"
Sleep 400ms
Enter
Sleep 700ms

# Type, then ask the language server for a completion.
Type "    let greeting = String::from(\"hi\");"
Sleep 500ms
Type "\n"
Type "    pri"
Sleep 300ms
Ctrl+Space
Sleep 1.6s
Escape

# Palette: git diff overlay, close it, then the save command.
F1
Type "git diff"
Sleep 500ms
Enter
Sleep 1.6s
Escape
Sleep 400ms
F1
Type "save"
Sleep 700ms
Escape
Sleep 300ms

# Save and leave.
Ctrl+s
Sleep 500ms
Ctrl+q
Sleep 800ms
Enter
Sleep 600ms
