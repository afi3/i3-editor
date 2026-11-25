//! MCP client core: stdio JSON-RPC transport, lifecycle, and tool calls.
//!
//! MCP uses newline-delimited JSON-RPC 2.0 messages over stdio. This module
//! deliberately has no UI or permission policy yet; the next slice wires the
//! `mcp` menu and trust prompts to these primitives.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

const MAX_MESSAGE_BYTES: usize = 8 << 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const LOG_LINES: usize = 200;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgKind {
    String,
    Integer,
    Number,
    Boolean,
    Enum(Vec<String>),
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolArg {
    pub name: String,
    pub description: String,
    pub kind: ArgKind,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl Tool {
    pub fn arguments(&self) -> Vec<ToolArg> {
        let required: Vec<&str> = self
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some(properties) = self
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };
        properties
            .iter()
            .map(|(name, schema)| {
                let description = schema
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let kind = match schema.get("enum").and_then(Value::as_array) {
                    Some(values) => ArgKind::Enum(
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect(),
                    ),
                    None => match schema.get("type").and_then(Value::as_str) {
                        Some("string") => ArgKind::String,
                        Some("integer") => ArgKind::Integer,
                        Some("number") => ArgKind::Number,
                        Some("boolean") => ArgKind::Boolean,
                        _ => ArgKind::Json,
                    },
                };
                ToolArg {
                    name: name.clone(),
                    description,
                    kind,
                    required: required.contains(&name.as_str()),
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    Text(String),
    Other(Value),
}

#[derive(Debug)]
pub enum Event {
    Tools {
        server: String,
        tools: Vec<Tool>,
    },
    ToolResult {
        server: String,
        tool: String,
        content: Vec<ToolContent>,
        is_error: bool,
    },
    Log {
        server: String,
        line: String,
    },
    Warning(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplyKind {
    Initialize,
    ToolsList,
    CallTool,
}

struct Pending {
    kind: ReplyKind,
    tool: Option<String>,
    sent: Instant,
}

struct Queued {
    method: &'static str,
    params: Value,
    kind: ReplyKind,
    tool: Option<String>,
}

enum Incoming {
    Line(String),
    Stderr(String),
    ProtocolError(String),
    Closed,
}

struct Server {
    child: Child,
    writer: ChildStdin,
    incoming: Receiver<Incoming>,
    initialized: bool,
    pending: HashMap<i64, Pending>,
    queued: VecDeque<Queued>,
    tools: Vec<Tool>,
    tools_loaded: bool,
    logs: VecDeque<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct McpManager {
    configs: BTreeMap<String, McpServerConfig>,
    root: PathBuf,
    servers: HashMap<String, Server>,
    failed: HashSet<String>,
    next_id: i64,
    request_timeout: Duration,
    initialize_timeout: Duration,
}

impl McpManager {
    pub fn new(configs: BTreeMap<String, McpServerConfig>, root: PathBuf) -> Self {
        Self {
            configs,
            root,
            servers: HashMap::new(),
            failed: HashSet::new(),
            next_id: 0,
            request_timeout: REQUEST_TIMEOUT,
            initialize_timeout: INITIALIZE_TIMEOUT,
        }
    }

    pub fn config_hash(&self, name: &str) -> Option<String> {
        let config = self.configs.get(name)?;
        let bytes = serde_json::to_vec(config).ok()?;
        let mut hash = Sha256::new();
        hash.update(&bytes);
        Some(format!("{:x}", hash.finalize()))
    }

    pub fn is_configured(&self) -> bool {
        !self.configs.is_empty()
    }

    pub fn is_running(&self, server: &str) -> bool {
        self.servers.contains_key(server)
    }

    pub fn names(&self) -> Vec<&str> {
        self.configs.keys().map(String::as_str).collect()
    }

    pub fn tools(&self, server: &str) -> Option<&[Tool]> {
        self.servers
            .get(server)
            .map(|server| server.tools.as_slice())
    }

    pub fn tools_loaded(&self, server: &str) -> bool {
        self.servers
            .get(server)
            .is_some_and(|server| server.tools_loaded)
    }

    pub fn logs(&self, server: &str) -> Option<&VecDeque<String>> {
        self.servers.get(server).map(|server| &server.logs)
    }

    pub fn failed(&self, server: &str) -> bool {
        self.failed.contains(server)
    }

    pub fn list_tools(&mut self, server: &str) -> Result<(), String> {
        self.request(server, "tools/list", json!({}), ReplyKind::ToolsList, None)
    }

    /// Stop a tool call by terminating the MCP server process. MCP has no
    /// mandatory request-cancellation acknowledgement, so killing the child is
    /// the only deterministic way to stop a tool that is already running.
    pub fn cancel_tool_call(&mut self, server: &str) -> bool {
        let Some(process) = self.servers.remove(server) else {
            return false;
        };
        drop(process);
        self.failed.remove(server);
        true
    }

    pub fn call_tool(&mut self, server: &str, tool: &str, arguments: Value) -> Result<(), String> {
        self.request(
            server,
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
            ReplyKind::CallTool,
            Some(tool.to_string()),
        )
    }

    #[cfg(test)]
    fn set_timeouts(&mut self, request: Duration, initialize: Duration) {
        self.request_timeout = request;
        self.initialize_timeout = initialize;
    }

    pub fn restart(&mut self, configs: BTreeMap<String, McpServerConfig>, root: PathBuf) {
        self.servers.clear();
        self.failed.clear();
        self.configs = configs;
        self.root = root;
    }

    pub fn pump(&mut self) -> Vec<Event> {
        let names: Vec<String> = self.servers.keys().cloned().collect();
        let mut events = Vec::new();
        for name in names {
            self.pump_server(&name, &mut events);
        }
        events
    }

    fn request(
        &mut self,
        name: &str,
        method: &'static str,
        params: Value,
        kind: ReplyKind,
        tool: Option<String>,
    ) -> Result<(), String> {
        if !self.servers.contains_key(name) {
            self.start(name)?;
        }
        let initialized = self
            .servers
            .get(name)
            .is_some_and(|server| server.initialized);
        if !initialized {
            if let Some(server) = self.servers.get_mut(name) {
                server.queued.push_back(Queued {
                    method,
                    params,
                    kind,
                    tool,
                });
            }
            return Ok(());
        }
        let id = self.next_id();
        let write = {
            let Some(server) = self.servers.get_mut(name) else {
                return Err(format!("MCP {name}: server disappeared"));
            };
            let write = write_request(&mut server.writer, id, method, params);
            if write.is_ok() {
                server.pending.insert(
                    id,
                    Pending {
                        kind,
                        tool,
                        sent: Instant::now(),
                    },
                );
            }
            write
        };
        if let Err(error) = write {
            self.servers.remove(name);
            self.failed.insert(name.to_string());
            return Err(error);
        }
        Ok(())
    }

    fn start(&mut self, name: &str) -> Result<(), String> {
        if self.servers.contains_key(name) {
            return Ok(());
        }
        if self.failed.contains(name) {
            return Err(format!("MCP {name}: server previously failed"));
        }
        let config = self
            .configs
            .get(name)
            .cloned()
            .ok_or_else(|| format!("MCP {name}: no configuration"))?;
        let mut server = match spawn_server(name, &config, &self.root) {
            Ok(server) => server,
            Err(error) => {
                self.failed.insert(name.to_string());
                return Err(error);
            }
        };
        let id = self.next_id();
        if let Err(error) = write_request(&mut server.writer, id, "initialize", initialize_params())
        {
            let _ = server.child.kill();
            let _ = server.child.wait();
            self.failed.insert(name.to_string());
            return Err(error);
        }
        server.pending.insert(
            id,
            Pending {
                kind: ReplyKind::Initialize,
                tool: None,
                sent: Instant::now(),
            },
        );
        self.servers.insert(name.to_string(), server);
        Ok(())
    }

    fn next_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn pump_server(&mut self, name: &str, events: &mut Vec<Event>) {
        let mut messages = Vec::new();
        if let Some(server) = self.servers.get(name) {
            while let Ok(message) = server.incoming.try_recv() {
                messages.push(message);
            }
        }
        let mut closed = false;
        for message in messages {
            match message {
                Incoming::Line(line) => self.handle_line(name, &line, events),
                Incoming::Stderr(line) => {
                    if let Some(server) = self.servers.get_mut(name) {
                        server.logs.push_back(line.clone());
                        if server.logs.len() > LOG_LINES {
                            server.logs.pop_front();
                        }
                    }
                    events.push(Event::Log {
                        server: name.to_string(),
                        line,
                    });
                }
                Incoming::ProtocolError(message) => {
                    events.push(Event::Warning(format!("MCP {name}: {message}")));
                    closed = true;
                }
                Incoming::Closed => closed = true,
            }
        }
        self.expire_pending(name, events);
        if closed {
            self.servers.remove(name);
            self.failed.insert(name.to_string());
            events.push(Event::Warning(format!("MCP {name}: server stopped")));
        }
    }

    fn expire_pending(&mut self, name: &str, events: &mut Vec<Event>) {
        let now = Instant::now();
        let mut expired = Vec::new();
        if let Some(server) = self.servers.get_mut(name) {
            server.pending.retain(|_, pending| {
                let timeout = if pending.kind == ReplyKind::Initialize {
                    self.initialize_timeout
                } else {
                    self.request_timeout
                };
                let keep = now.duration_since(pending.sent) <= timeout;
                if !keep {
                    expired.push(pending.kind);
                }
                keep
            });
        }
        for kind in expired {
            let _ = kind;
            events.push(Event::Warning(format!("MCP {name}: request timed out")));
        }
    }

    fn handle_line(&mut self, name: &str, line: &str, events: &mut Vec<Event>) {
        if line.len() > MAX_MESSAGE_BYTES {
            events.push(Event::Warning(format!("MCP {name}: response too large")));
            return;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                events.push(Event::Warning(format!("MCP {name}: invalid JSON: {error}")));
                return;
            }
        };
        if let Some(id) = value.get("id").and_then(Value::as_i64) {
            let Some(pending) = self
                .servers
                .get_mut(name)
                .and_then(|server| server.pending.remove(&id))
            else {
                return;
            };
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                events.push(Event::Warning(format!("MCP {name}: {message}")));
                return;
            }
            let result = value.get("result").cloned().unwrap_or(Value::Null);
            match pending.kind {
                ReplyKind::Initialize => self.finish_initialize(name, events),
                ReplyKind::ToolsList => {
                    let tools = parse_tools(&result);
                    if let Some(server) = self.servers.get_mut(name) {
                        server.tools = tools.clone();
                        server.tools_loaded = true;
                    }
                    events.push(Event::Tools {
                        server: name.to_string(),
                        tools,
                    });
                }
                ReplyKind::CallTool => {
                    let (content, is_error) = parse_tool_result(&result);
                    events.push(Event::ToolResult {
                        server: name.to_string(),
                        tool: pending.tool.unwrap_or_default(),
                        content,
                        is_error,
                    });
                }
            }
            return;
        }
        if value.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed") {
            events.push(Event::Warning(format!(
                "MCP {name}: tool list changed; refresh tools"
            )));
        }
    }

    fn finish_initialize(&mut self, name: &str, events: &mut Vec<Event>) {
        let queued = {
            let Some(server) = self.servers.get_mut(name) else {
                return;
            };
            server.initialized = true;
            if let Err(error) = write_notification(&mut server.writer, "notifications/initialized")
            {
                events.push(Event::Warning(format!("MCP {name}: {error}")));
            }
            std::mem::take(&mut server.queued)
        };
        for request in queued {
            if let Err(error) = self.request(
                name,
                request.method,
                request.params,
                request.kind,
                request.tool,
            ) {
                events.push(Event::Warning(error));
            }
        }
    }
}

fn spawn_server(name: &str, config: &McpServerConfig, root: &Path) -> Result<Server, String> {
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .current_dir(config.cwd.as_deref().unwrap_or(root))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.envs(&config.env);
    let mut child = command
        .spawn()
        .map_err(|error| format!("MCP {name}: cannot start {}: {error}", config.command))?;
    let writer = child
        .stdin
        .take()
        .ok_or_else(|| format!("MCP {name}: no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("MCP {name}: no stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("MCP {name}: no stderr"))?;
    let (tx, incoming) = mpsc::channel();
    let stdout_tx = tx.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) if line.len() > MAX_MESSAGE_BYTES => {
                    let _ = stdout_tx.send(Incoming::ProtocolError("message too large".into()));
                    break;
                }
                Ok(_) => {
                    if stdout_tx.send(Incoming::Line(line.clone())).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = stdout_tx.send(Incoming::ProtocolError(error.to_string()));
                    break;
                }
            }
        }
        let _ = stdout_tx.send(Incoming::Closed);
    });
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(Incoming::Stderr(line)).is_err() {
                break;
            }
        }
    });
    Ok(Server {
        child,
        writer,
        incoming,
        initialized: false,
        pending: HashMap::new(),
        queued: VecDeque::new(),
        tools: Vec::new(),
        tools_loaded: false,
        logs: VecDeque::new(),
    })
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "i3-editor",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn write_request(
    writer: &mut ChildStdin,
    id: i64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    write_message(
        writer,
        &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
}

fn write_notification(writer: &mut ChildStdin, method: &str) -> Result<(), String> {
    write_message(writer, &json!({"jsonrpc": "2.0", "method": method}))
}

fn write_message(writer: &mut ChildStdin, message: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string(message).map_err(|error| error.to_string())?;
    text.push('\n');
    writer
        .write_all(text.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|error| error.to_string())
}

fn parse_tools(result: &Value) -> Vec<Tool> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name")?.as_str()?.to_string();
                    Some(Tool {
                        name,
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_tool_result(result: &Value) -> (Vec<ToolContent>, bool) {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    if item.get("type").and_then(Value::as_str) == Some("text") {
                        ToolContent::Text(
                            item.get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    } else {
                        ToolContent::Other(item.clone())
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (content, is_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mock(mode: &str) -> McpManager {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("mock_mcp.py");
        let mut env = BTreeMap::new();
        env.insert("MOCK_MCP_MODE".to_string(), mode.to_string());
        let config = McpServerConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: None,
            env,
        };
        McpManager::new(
            BTreeMap::from([("mock".to_string(), config)]),
            std::env::temp_dir(),
        )
    }

    fn python_available() -> bool {
        Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    fn wait_for<F>(manager: &mut McpManager, mut predicate: F) -> Vec<Event>
    where
        F: FnMut(&Event) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut seen = Vec::new();
        let mut pending = VecDeque::new();
        while Instant::now() < deadline {
            if let Some(event) = pending.pop_front() {
                if predicate(&event) {
                    seen.push(event);
                    return seen;
                }
                seen.push(event);
                continue;
            }
            pending.extend(manager.pump());
            if pending.is_empty() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        panic!("timed out waiting for MCP event; seen {seen:?}");
    }

    #[test]
    fn initialize_lists_tools_and_calls_one() {
        if !python_available() {
            eprintln!("skipping MCP test: python3 unavailable");
            return;
        }
        let mut manager = mock("normal");
        manager.list_tools("mock").expect("list tools");
        let events = wait_for(&mut manager, |event| matches!(event, Event::Tools { .. }));
        let tools = events
            .iter()
            .find_map(|event| match event {
                Event::Tools { tools, .. } => Some(tools.clone()),
                _ => None,
            })
            .expect("tools event");
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[1].name, "add");
        assert_eq!(tools[2].name, "read_text_file");
        assert_eq!(manager.tools("mock").expect("stored tools").len(), 3);

        manager
            .call_tool("mock", "echo", json!({"text": "hello"}))
            .expect("call tool");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::ToolResult { .. })
        });
        let result = events
            .iter()
            .find_map(|event| match event {
                Event::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .expect("tool result");
        assert_eq!(result, vec![ToolContent::Text("hello".to_string())]);
    }

    #[test]
    fn tool_schema_becomes_required_and_typed_arguments() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path"},
                "line": {"type": "integer"},
                "mode": {"type": "string", "enum": ["read", "write"]},
                "force": {"type": "boolean"}
            },
            "required": ["path"]
        });
        let tool = Tool {
            name: "demo".to_string(),
            description: String::new(),
            input_schema: schema,
        };
        let args = tool.arguments();
        assert_eq!(args.len(), 4);
        let path = args.iter().find(|arg| arg.name == "path").expect("path");
        assert_eq!(path.kind, ArgKind::String);
        assert!(path.required);
        assert!(
            !args
                .iter()
                .find(|arg| arg.name == "line")
                .expect("line")
                .required
        );
        assert_eq!(
            args.iter()
                .find(|arg| arg.name == "mode")
                .expect("mode")
                .kind,
            ArgKind::Enum(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(
            args.iter()
                .find(|arg| arg.name == "force")
                .expect("force")
                .kind,
            ArgKind::Boolean
        );
    }

    #[test]
    fn malformed_server_output_is_reported_without_hanging() {
        if !python_available() {
            eprintln!("skipping MCP test: python3 unavailable");
            return;
        }
        let mut manager = mock("malformed");
        manager.list_tools("mock").expect("list tools");
        let events = wait_for(
            &mut manager,
            |event| matches!(event, Event::Warning(message) if message.contains("invalid JSON")),
        );
        assert!(events.iter().any(
            |event| matches!(event, Event::Warning(message) if message.contains("invalid JSON"))
        ));
    }

    #[test]
    fn crashed_server_is_reported_once() {
        if !python_available() {
            eprintln!("skipping MCP test: python3 unavailable");
            return;
        }
        let mut manager = mock("crash");
        manager.list_tools("mock").expect("start server");
        let events = wait_for(
            &mut manager,
            |event| matches!(event, Event::Warning(message) if message.contains("server stopped")),
        );
        assert!(manager.failed("mock"));
        assert!(events.iter().any(
            |event| matches!(event, Event::Warning(message) if message.contains("server stopped"))
        ));
    }
    #[test]
    fn silent_server_times_out() {
        if !python_available() {
            eprintln!("skipping MCP test: python3 unavailable");
            return;
        }
        let mut manager = mock("silent");
        manager.set_timeouts(Duration::from_millis(50), Duration::from_millis(50));
        manager.list_tools("mock").expect("start server");
        let events = wait_for(
            &mut manager,
            |event| matches!(event, Event::Warning(message) if message.contains("timed out")),
        );
        assert!(events.iter().any(
            |event| matches!(event, Event::Warning(message) if message.contains("timed out"))
        ));
    }

    #[test]
    fn oversized_server_output_is_reported() {
        if !python_available() {
            eprintln!("skipping MCP test: python3 unavailable");
            return;
        }
        let mut manager = mock("oversized");
        manager.list_tools("mock").expect("list tools");
        let events = wait_for(
            &mut manager,
            |event| matches!(event, Event::Warning(message) if message.contains("too large")),
        );
        assert!(events.iter().any(
            |event| matches!(event, Event::Warning(message) if message.contains("too large"))
        ));
    }
    #[test]
    #[ignore = "requires npx, network, and the filesystem MCP package"]
    fn real_filesystem_mcp_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("hello.txt"), "hello").expect("write");
        let config = McpServerConfig {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                dir.path().to_string_lossy().into_owned(),
            ],
            cwd: None,
            env: BTreeMap::new(),
        };
        let mut manager = McpManager::new(
            BTreeMap::from([("filesystem".to_string(), config)]),
            dir.path().to_path_buf(),
        );
        manager.list_tools("filesystem").expect("start server");
        let events = wait_for(&mut manager, |event| matches!(event, Event::Tools { .. }));
        let tools = events
            .iter()
            .find_map(|event| match event {
                Event::Tools { tools, .. } => Some(tools.clone()),
                _ => None,
            })
            .expect("tools");
        println!(
            "tools: {:?}",
            tools.iter().map(|tool| &tool.name).collect::<Vec<_>>()
        );
        assert!(tools.iter().any(|tool| tool.name == "list_directory"));

        manager
            .call_tool(
                "filesystem",
                "list_directory",
                json!({"path": dir.path().to_string_lossy()}),
            )
            .expect("call");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::ToolResult { .. })
        });
        let result = events
            .iter()
            .find_map(|event| match event {
                Event::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .expect("result");
        println!("result: {result:?}");
        assert!(
            result
                .iter()
                .any(|item| matches!(item, ToolContent::Text(text) if text.contains("hello.txt")))
        );
    }
}
