-- Demo plugin: a save hook that writes a marker.
-- Requires `[lua] allow_file = true`; the path is relative to the editor cwd.
events.on("save", function()
  if fs then
    fs.write_file("i3-editor-save-marker", "saved " .. (editor.path() or "?"))
  end
end)
