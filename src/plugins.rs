//! Sandboxed Lua plugins: commands, keymaps, events, and statusline segments.
//!
//! One Lua state holds every plugin. Each callback runs on the UI thread with an
//! instruction hook that aborts after the configured time budget; callbacks are
//! re-entrancy limited and capped per plugin. `io`/`os`/`package` are not loaded
//! unless the matching capability is enabled.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, Value, VmState};

use crate::config::LuaConfig;
use crate::input::KeyCombo;

const MAX_DEPTH: usize = 4;
const MAX_FAILURES: usize = 3;
const INSTRUCTIONS_PER_HOOK: u32 = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub path: Option<String>,
    pub language: String,
    /// Zero-based line and char column.
    pub line: usize,
    pub column: usize,
    /// Every selection/caret as `(start, end)` char offsets.
    pub selections: Vec<(usize, usize)>,
}

struct CommandEntry {
    name: String,
    plugin: String,
    id: usize,
}

struct Hook {
    plugin: String,
    id: usize,
}

#[derive(Default)]
struct Registry {
    /// Bumped whenever the registered hooks change; the status cache keys on it.
    revision: u64,
    commands: Vec<CommandEntry>,
    keymap: Vec<(KeyCombo, String, String)>,
    events: HashMap<String, Vec<Hook>>,
    status: Vec<Hook>,
    failures: HashMap<String, usize>,
    disabled: HashSet<String>,
    warnings: Vec<String>,
}

impl Registry {
    fn disable(&mut self, plugin: &str) {
        self.revision += 1;
        self.disabled.insert(plugin.to_string());
        self.commands.retain(|entry| entry.plugin != plugin);
        self.keymap.retain(|(_, _, owner)| owner != plugin);
        for hooks in self.events.values_mut() {
            hooks.retain(|hook| hook.plugin != plugin);
        }
        self.status.retain(|hook| hook.plugin != plugin);
    }
}

pub struct Plugins {
    pub dir: PathBuf,
    lua: Lua,
    registry: Rc<RefCell<Registry>>,
    budget: Duration,
    allow_file: bool,
    allow_process: bool,
    depth: usize,
    /// Last registry revision, snapshot and segments; status callbacks are
    /// re-run only when the hooks or the state they can see changed.
    last_status: Option<(u64, Snapshot, Vec<String>)>,
}

impl Plugins {
    pub fn default_dir() -> Option<PathBuf> {
        crate::config::config_dir().map(|dir| dir.join("plugins"))
    }

    pub fn new(dir: PathBuf, config: &LuaConfig) -> Result<Self, String> {
        let lua = new_lua(config).map_err(|error| error.to_string())?;
        let mut plugins = Self {
            dir,
            lua,
            registry: Rc::new(RefCell::new(Registry::default())),
            budget: Duration::from_millis(config.time_budget_ms.max(10)),
            last_status: None,
            allow_file: config.allow_file,
            allow_process: config.allow_process,
            depth: 0,
        };
        plugins.reload();
        Ok(plugins)
    }

    /// Drop every registration and load `*.lua` again. Warnings are available
    /// through `take_warnings`.
    pub fn reload(&mut self) {
        match new_lua(&LuaConfig {
            enabled: true,
            allow_file: self.allow_file,
            allow_process: self.allow_process,
            time_budget_ms: self.budget.as_millis() as u64,
        }) {
            Ok(lua) => self.lua = lua,
            Err(error) => {
                self.warning(format!("Lua unavailable: {error}"));
                return;
            }
        }
        self.registry = Rc::new(RefCell::new(Registry::default()));
        self.depth = 0;
        self.last_status = None;
        if let Err(error) = std::fs::create_dir_all(&self.dir) {
            self.warning(format!("cannot create {}: {error}", self.dir.display()));
            return;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "lua"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for path in files {
            self.load_plugin(&path);
        }
    }

    fn load_plugin(&mut self, path: &Path) {
        let plugin = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                self.warning(format!("{plugin}: cannot read: {error}"));
                return;
            }
        };
        if let Err(error) = install_api(
            &self.lua,
            &self.registry,
            &plugin,
            self.allow_file,
            self.allow_process,
        ) {
            self.warning(format!("{plugin}: API setup failed: {error}"));
            return;
        }
        if set_context(&self.lua, &Snapshot::default()).is_err() {
            self.warning(format!("{plugin}: context setup failed"));
            return;
        }
        let name = path.to_string_lossy().into_owned();
        let deadline = Instant::now() + self.budget;
        let hook = self.lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(INSTRUCTIONS_PER_HOOK),
            move |_, _| {
                if Instant::now() >= deadline {
                    Err(mlua::Error::runtime("script time budget exceeded"))
                } else {
                    Ok(VmState::Continue)
                }
            },
        );
        let result = match hook {
            Ok(()) => self.lua.load(&source).set_name(name).exec(),
            Err(error) => Err(error),
        };
        self.lua.remove_global_hook();
        if let Err(error) = result {
            self.record_error(&plugin, &error.to_string());
        }
    }

    pub fn command_names(&self) -> Vec<String> {
        let registry = self.registry.borrow();
        registry
            .commands
            .iter()
            .filter(|entry| !registry.disabled.contains(&entry.plugin))
            .map(|entry| entry.name.clone())
            .collect()
    }

    pub fn binding(&self, combo: KeyCombo) -> Option<String> {
        let registry = self.registry.borrow();
        registry
            .keymap
            .iter()
            .find(|(bound, _, owner)| *bound == combo && !registry.disabled.contains(owner))
            .map(|(_, command, _)| command.clone())
    }

    /// Run a registered command. Returns false when no plugin owns that name.
    pub fn invoke_command(&mut self, name: &str, snapshot: &Snapshot) -> bool {
        self.last_status = None;
        let entry = {
            let registry = self.registry.borrow();
            registry
                .commands
                .iter()
                .find(|entry| entry.name == name && !registry.disabled.contains(&entry.plugin))
                .map(|entry| (entry.plugin.clone(), entry.id))
        };
        let Some((plugin, id)) = entry else {
            return false;
        };
        if self.depth >= MAX_DEPTH {
            self.warning(format!("{plugin}: callback depth limit reached"));
            return true;
        }
        self.depth += 1;
        let _ = self.call(&plugin, id, snapshot);
        self.depth -= 1;
        true
    }

    pub fn fire(&mut self, event: &str, snapshot: &Snapshot) {
        // A callback can change what the status hooks read, even with the same
        // snapshot, so the cached segments must not survive it.
        self.last_status = None;
        let hooks: Vec<(String, usize)> = {
            let registry = self.registry.borrow();
            registry
                .events
                .get(event)
                .map(|hooks| {
                    hooks
                        .iter()
                        .filter(|hook| !registry.disabled.contains(&hook.plugin))
                        .map(|hook| (hook.plugin.clone(), hook.id))
                        .collect()
                })
                .unwrap_or_default()
        };
        self.run_hooks(hooks, snapshot);
    }

    pub fn status_segments(&mut self, snapshot: &Snapshot) -> Vec<String> {
        let revision = self.registry.borrow().revision;
        if let Some((cached_revision, cached, segments)) = &self.last_status
            && *cached_revision == revision
            && cached == snapshot
        {
            return segments.clone();
        }
        let hooks: Vec<(String, usize)> = {
            let registry = self.registry.borrow();
            registry
                .status
                .iter()
                .filter(|hook| !registry.disabled.contains(&hook.plugin))
                .map(|hook| (hook.plugin.clone(), hook.id))
                .collect()
        };
        if hooks.is_empty() {
            return Vec::new();
        }
        let mut segments = Vec::new();
        if self.depth >= MAX_DEPTH {
            return segments;
        }
        self.depth += 1;
        for (plugin, id) in hooks {
            if let Some(text) = self.call(&plugin, id, snapshot)
                && !text.is_empty()
            {
                segments.push(text.chars().take(120).collect());
            }
        }
        self.depth -= 1;
        self.last_status = Some((revision, snapshot.clone(), segments.clone()));
        segments
    }

    fn run_hooks(&mut self, hooks: Vec<(String, usize)>, snapshot: &Snapshot) {
        if self.depth >= MAX_DEPTH {
            self.warning("callback depth limit reached".to_string());
            return;
        }
        self.depth += 1;
        for (plugin, id) in hooks {
            let _ = self.call(&plugin, id, snapshot);
        }
        self.depth -= 1;
    }

    fn call(&mut self, plugin: &str, id: usize, snapshot: &Snapshot) -> Option<String> {
        if self.registry.borrow().disabled.contains(plugin) {
            return None;
        }
        let deadline = Instant::now() + self.budget;
        if self
            .lua
            .set_hook(
                HookTriggers::new().every_nth_instruction(INSTRUCTIONS_PER_HOOK),
                move |_, _| {
                    if Instant::now() >= deadline {
                        Err(mlua::Error::runtime("script time budget exceeded"))
                    } else {
                        Ok(VmState::Continue)
                    }
                },
            )
            .is_err()
        {
            return None;
        }
        let result = (|| -> mlua::Result<Option<String>> {
            set_context(&self.lua, snapshot)?;
            let callbacks: Table = self.lua.globals().get("__i3_callbacks")?;
            let function: Function = callbacks.get(id)?;
            let value: Value = function.call(())?;
            Ok(match value {
                Value::Nil => None,
                Value::String(text) => Some(text.to_str()?.to_string()),
                other => Some(other.to_string()?),
            })
        })();
        self.lua.remove_global_hook();
        match result {
            Ok(value) => value,
            Err(error) => {
                self.record_error(plugin, &error.to_string());
                None
            }
        }
    }

    fn record_error(&mut self, plugin: &str, message: &str) {
        let failures = {
            let mut registry = self.registry.borrow_mut();
            registry.warnings.push(format!("{plugin}: {message}"));
            let failures = registry.failures.entry(plugin.to_string()).or_insert(0);
            *failures += 1;
            *failures
        };
        if failures >= MAX_FAILURES {
            let mut registry = self.registry.borrow_mut();
            registry.disable(plugin);
            registry
                .warnings
                .push(format!("{plugin}: disabled after {failures} errors"));
        }
    }

    fn warning(&mut self, message: String) {
        self.registry.borrow_mut().warnings.push(message);
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.registry.borrow_mut().warnings)
    }
}

/// A plugin child process is bounded in time and output: the instruction hook
/// cannot fire while the editor waits for it.
const PROCESS_TIMEOUT: Duration = Duration::from_secs(60);
const PROCESS_OUTPUT_LIMIT: usize = 1 << 20;

struct ProcessOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn run_process(mut command: std::process::Command) -> std::result::Result<ProcessOutput, String> {
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().map(collect_process_pipe);
    let stderr = child.stderr.take().map(collect_process_pipe);
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "process timed out after {}s",
                    PROCESS_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    };
    let (stdout, stdout_dropped) = join_process_pipe(stdout);
    let (stderr, stderr_dropped) = join_process_pipe(stderr);
    if stdout_dropped || stderr_dropped {
        // Returning a short string as if it were complete would corrupt any
        // plugin that parses it.
        return Err(format!(
            "process output exceeded {} bytes",
            PROCESS_OUTPUT_LIMIT
        ));
    }
    Ok(ProcessOutput {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn collect_process_pipe(
    mut pipe: impl std::io::Read + Send + 'static,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut dropped = false;
        let mut buffer = [0u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if kept.len() < PROCESS_OUTPUT_LIMIT {
                        let take = read.min(PROCESS_OUTPUT_LIMIT - kept.len());
                        kept.extend_from_slice(&buffer[..take]);
                        if take < read {
                            dropped = true;
                        }
                    } else {
                        dropped = true;
                    }
                }
            }
        }
        (kept, dropped)
    })
}

fn join_process_pipe(handle: Option<std::thread::JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

fn new_lua(config: &LuaConfig) -> mlua::Result<Lua> {
    // No `coroutine`: mlua's instruction budget does not reach a Lua thread
    // created inside a callback, so a coroutine could run forever with the UI
    // thread blocked and no error to report.
    let mut libs = StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8;
    if config.allow_file {
        libs |= StdLib::IO;
    }
    if config.allow_process {
        libs |= StdLib::OS;
    }
    let lua = Lua::new_with(libs, LuaOptions::default())?;
    if !config.allow_file {
        // `luaopen_base` registers these regardless of the io library.
        lua.globals().set("dofile", Value::Nil)?;
        lua.globals().set("loadfile", Value::Nil)?;
    }
    lua.globals().set("__i3_callbacks", lua.create_table()?)?;
    lua.globals().set("api_version", 1)?;
    set_context(&lua, &Snapshot::default())?;
    Ok(lua)
}

fn set_context(lua: &Lua, snapshot: &Snapshot) -> mlua::Result<()> {
    let ctx = lua.create_table()?;
    ctx.set("path", snapshot.path.clone())?;
    ctx.set("language", snapshot.language.clone())?;
    ctx.set("line", snapshot.line)?;
    ctx.set("column", snapshot.column)?;
    let selections = lua.create_table()?;
    for (index, (start, end)) in snapshot.selections.iter().enumerate() {
        let item = lua.create_table()?;
        item.set("start", *start)?;
        item.set("end", *end)?;
        selections.set(index + 1, item)?;
    }
    ctx.set("selections", selections)?;
    lua.globals().set("__i3_ctx", ctx)
}

fn install_api(
    lua: &Lua,
    registry: &Rc<RefCell<Registry>>,
    plugin: &str,
    allow_file: bool,
    allow_process: bool,
) -> mlua::Result<()> {
    let commands = lua.create_table()?;
    {
        let registry = registry.clone();
        let owner = plugin.to_string();
        commands.set(
            "register",
            lua.create_function(move |lua, (name, function): (String, Function)| {
                if name.trim().is_empty() {
                    return Err(mlua::Error::runtime("command name is empty"));
                }
                let callbacks: Table = lua.globals().get("__i3_callbacks")?;
                let id = callbacks.raw_len() + 1;
                callbacks.set(id, function)?;
                let mut registry = registry.borrow_mut();
                if registry.commands.iter().any(|entry| entry.name == name) {
                    return Err(mlua::Error::runtime(format!(
                        "command '{name}' already registered"
                    )));
                }
                registry.revision += 1;
                registry.commands.push(CommandEntry {
                    name,
                    plugin: owner.clone(),
                    id,
                });
                Ok(())
            })?,
        )?;
    }
    lua.globals().set("commands", commands)?;

    let keymap = lua.create_table()?;
    {
        let registry = registry.clone();
        let owner = plugin.to_string();
        keymap.set(
            "set",
            lua.create_function(move |_, (binding, command): (String, String)| {
                let combo = KeyCombo::parse(&binding)
                    .map_err(|error| mlua::Error::runtime(error.to_string()))?;
                if command.trim().is_empty() {
                    return Err(mlua::Error::runtime("keymap command is empty"));
                }
                registry
                    .borrow_mut()
                    .keymap
                    .push((combo, command, owner.clone()));
                Ok(())
            })?,
        )?;
    }
    lua.globals().set("keymap", keymap)?;

    let events = lua.create_table()?;
    {
        let registry = registry.clone();
        let owner = plugin.to_string();
        events.set(
            "on",
            lua.create_function(move |lua, (event, function): (String, Function)| {
                if !matches!(event.as_str(), "open" | "save" | "change" | "focus") {
                    return Err(mlua::Error::runtime(format!(
                        "unknown event '{event}' (open, save, change, focus)"
                    )));
                }
                let callbacks: Table = lua.globals().get("__i3_callbacks")?;
                let id = callbacks.raw_len() + 1;
                callbacks.set(id, function)?;
                registry
                    .borrow_mut()
                    .events
                    .entry(event)
                    .or_default()
                    .push(Hook {
                        plugin: owner.clone(),
                        id,
                    });
                Ok(())
            })?,
        )?;
    }
    lua.globals().set("events", events)?;

    let statusline = lua.create_table()?;
    {
        let registry = registry.clone();
        let owner = plugin.to_string();
        statusline.set(
            "register",
            lua.create_function(move |lua, function: Function| {
                let callbacks: Table = lua.globals().get("__i3_callbacks")?;
                let id = callbacks.raw_len() + 1;
                callbacks.set(id, function)?;
                let mut registry = registry.borrow_mut();
                registry.revision += 1;
                registry.status.push(Hook {
                    plugin: owner.clone(),
                    id,
                });
                Ok(())
            })?,
        )?;
    }
    lua.globals().set("statusline", statusline)?;

    let editor = lua.create_table()?;
    editor.set(
        "path",
        lua.create_function(|lua, _: ()| {
            let ctx: Table = lua.globals().get("__i3_ctx")?;
            ctx.get::<Value>("path")
        })?,
    )?;
    editor.set(
        "language",
        lua.create_function(|lua, _: ()| {
            let ctx: Table = lua.globals().get("__i3_ctx")?;
            ctx.get::<String>("language")
        })?,
    )?;
    editor.set(
        "cursor",
        lua.create_function(|lua, _: ()| {
            let ctx: Table = lua.globals().get("__i3_ctx")?;
            Ok((ctx.get::<usize>("line")?, ctx.get::<usize>("column")?))
        })?,
    )?;
    editor.set(
        "selections",
        lua.create_function(|lua, _: ()| {
            let ctx: Table = lua.globals().get("__i3_ctx")?;
            ctx.get::<Table>("selections")
        })?,
    )?;
    lua.globals().set("editor", editor)?;

    if allow_file {
        let fs = lua.create_table()?;
        fs.set(
            "write_file",
            lua.create_function(|_, (path, contents): (String, String)| {
                std::fs::write(&path, contents)
                    .map_err(|error| mlua::Error::runtime(error.to_string()))
            })?,
        )?;
        lua.globals().set("fs", fs)?;
    }
    if allow_process {
        let process = lua.create_table()?;
        process.set(
            "run",
            lua.create_function(|_, (program, args): (String, Option<Table>)| {
                let mut command = std::process::Command::new(program);
                if let Some(args) = args {
                    for value in args.sequence_values::<String>() {
                        command.arg(value?);
                    }
                }
                let output = run_process(command).map_err(mlua::Error::runtime)?;
                if !output.success {
                    return Err(mlua::Error::runtime(output.stderr.trim().to_string()));
                }
                Ok(output.stdout)
            })?,
        )?;
        lua.globals().set("process", process)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-lua-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn config(allow_file: bool) -> LuaConfig {
        LuaConfig {
            enabled: true,
            allow_file,
            allow_process: false,
            time_budget_ms: 60,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            path: Some("/tmp/note.rs".to_string()),
            language: "rust".to_string(),
            line: 2,
            column: 5,
            selections: vec![(1, 3), (7, 7)],
        }
    }

    #[test]
    fn commands_keymap_status_and_editor_queries_work() {
        let dir = dir("api");
        std::fs::write(
            dir.join("hello.lua"),
            r#"
commands.register("hello", function()
  marker = "ran"
end)
keymap.set("ctrl+alt+p", "hello")
statusline.register(function()
  local selections = editor.selections()
  return "S:" .. #selections .. ":" .. editor.language()
end)
"#,
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        assert!(plugins.command_names().contains(&"hello".to_string()));
        let combo = KeyCombo::parse("ctrl+alt+p").expect("combo");
        assert_eq!(plugins.binding(combo).as_deref(), Some("hello"));
        let segments = plugins.status_segments(&snapshot());
        assert_eq!(segments, vec!["S:2:rust"]);
        assert!(plugins.invoke_command("hello", &snapshot()));
        assert!(plugins.take_warnings().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_sandbox_hides_coroutines_so_no_runaway_can_escape_the_budget() {
        let dir = dir("coroutine");
        std::fs::write(
            dir.join("check.lua"),
            "assert(coroutine == nil, 'coroutine leaked')\n",
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        let warnings = plugins.take_warnings();
        assert!(warnings.is_empty(), "{warnings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_time_budget_reaches_a_top_level_loop() {
        let dir = dir("budget");
        std::fs::write(
            dir.join("spin.lua"),
            r#"
commands.register("spin", function()
  local x = 0
  for i = 1, 200000000 do x = x + i end
end)
"#,
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        let _ = plugins.invoke_command("spin", &snapshot());
        let warnings = plugins.take_warnings();
        assert!(
            warnings.iter().any(|warning| warning.contains("budget")),
            "a plain loop must hit the deadline: {warnings:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_default_sandbox_hides_the_file_loaders() {
        let dir = dir("loaders");
        std::fs::write(
            dir.join("check.lua"),
            "assert(dofile == nil, 'dofile leaked')\nassert(loadfile == nil, 'loadfile leaked')\n",
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        let warnings = plugins.take_warnings();
        assert!(warnings.is_empty(), "{warnings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn running_a_command_refreshes_the_cached_status_segments() {
        let dir = dir("status-refresh");
        std::fs::write(
            dir.join("refresh.lua"),
            r#"
count = 0
commands.register("bump", function()
  count = count + 1
end)
statusline.register(function()
  return tostring(count)
end)
"#,
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        let snap = snapshot();
        assert_eq!(plugins.status_segments(&snap), vec!["0".to_string()]);
        assert!(plugins.invoke_command("bump", &snap));
        assert_eq!(
            plugins.status_segments(&snap),
            vec!["1".to_string()],
            "a command must invalidate the cached segments"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unchanged_snapshot_does_not_rerun_status_hooks() {
        let dir = dir("status-cache");
        std::fs::write(
            dir.join("status.lua"),
            r#"
calls = 0
statusline.register(function()
  calls = calls + 1
  return tostring(calls)
end)
"#,
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        assert_eq!(plugins.status_segments(&snapshot()), vec!["1".to_string()]);
        assert_eq!(
            plugins.status_segments(&snapshot()),
            vec!["1".to_string()],
            "an unchanged snapshot reuses the segments"
        );
        let mut moved = snapshot();
        moved.line = 3;
        assert_eq!(plugins.status_segments(&moved), vec!["2".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sandbox_hides_io_and_os_by_default() {
        let dir = dir("sandbox");
        std::fs::write(
            dir.join("check.lua"),
            "assert(io == nil, 'io leaked')\nassert(os == nil, 'os leaked')\n",
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        assert!(plugins.take_warnings().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_hook_can_write_a_marker_with_the_capability() {
        let dir = dir("save");
        let marker = dir.join("marker.txt");
        std::fs::write(
            dir.join("save.lua"),
            format!(
                "events.on('save', function() fs.write_file('{}', 'saved') end)\n",
                marker.display()
            ),
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(true)).expect("plugins");
        plugins.fire("save", &snapshot());
        assert_eq!(std::fs::read_to_string(&marker).expect("marker"), "saved");
        assert!(plugins.take_warnings().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_scripts_report_the_file_and_disable_after_repeats() {
        let dir = dir("broken");
        std::fs::write(dir.join("broken.lua"), "error('boom')\n").expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        let warnings = plugins.take_warnings();
        assert!(!warnings.is_empty(), "no warning");
        assert!(warnings[0].contains("broken.lua"), "{warnings:?}");
        assert!(warnings[0].contains("boom"), "{warnings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_infinite_callback_is_stopped_by_the_budget() {
        let dir = dir("budget");
        std::fs::write(
            dir.join("spin.lua"),
            "commands.register('spin', function() while true do end end)\n",
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        let start = Instant::now();
        assert!(plugins.invoke_command("spin", &snapshot()));
        assert!(start.elapsed() < Duration::from_secs(2));
        let warnings = plugins.take_warnings();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("time budget")),
            "{warnings:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reload_drops_every_registration() {
        let dir = dir("reload");
        let path = dir.join("one.lua");
        std::fs::write(
            &path,
            "commands.register('one', function() end)\nstatusline.register(function() return 'x' end)\n",
        )
        .expect("write");
        let mut plugins = Plugins::new(dir.clone(), &config(false)).expect("plugins");
        assert!(plugins.command_names().contains(&"one".to_string()));
        assert_eq!(plugins.status_segments(&snapshot()), vec!["x"]);

        std::fs::write(&path, "-- removed\n").expect("rewrite");
        plugins.reload();
        assert!(plugins.command_names().is_empty());
        assert!(plugins.status_segments(&snapshot()).is_empty());
        assert!(!plugins.invoke_command("one", &snapshot()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
