-- Demo plugin: a palette command and a key binding.
-- Copy this file to <config>/i3-editor/plugins/hello.lua.
commands.register("plugin_hello", function()
  -- Callbacks run on the UI thread; keep them short.
  statusline.register(function()
    return "hello from Lua"
  end)
end)

keymap.set("ctrl+alt+h", "plugin_hello")
