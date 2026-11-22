-- Demo plugin: one statusline segment.
-- Shows the active language, line, and selection count.
statusline.register(function()
  local selections = editor.selections()
  local line = editor.cursor()
  return string.format("%s:%d [%d]", editor.language(), line + 1, #selections)
end)
