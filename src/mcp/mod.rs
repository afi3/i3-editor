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
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
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
    /// HTTP MCP endpoint. When set, command/args are not spawned.
    pub url: Option<String>,
    /// Advertise MCP sampling and allow server-initiated model requests.
    pub allow_sampling: bool,
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
    pub read_only: bool,
    pub destructive: bool,
    pub open_world: bool,
}

impl Tool {
    pub fn capability(&self) -> &'static str {
        if self.read_only {
            "read-only"
        } else if self.destructive {
            "destructive"
        } else {
            "write"
        }
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub name: String,
    pub description: String,
    pub arguments: Vec<String>,
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
    Resources {
        server: String,
        resources: Vec<Resource>,
    },
    Resource {
        server: String,
        uri: String,
        content: String,
        mime_type: String,
    },
    ResourceTemplates {
        server: String,
        templates: Vec<ResourceTemplate>,
    },
    Prompts {
        server: String,
        prompts: Vec<Prompt>,
    },
    Prompt {
        server: String,
        name: String,
        content: String,
    },
    Sampling {
        server: String,
        id: i64,
        params: Value,
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
    ResourcesList,
    ResourceRead,
    ResourceTemplatesList,
    PromptsList,
    PromptGet,
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

enum Transport {
    Stdio {
        child: Child,
        writer: ChildStdin,
    },
    Http {
        url: String,
        session: Arc<Mutex<Option<String>>>,
    },
}

struct Server {
    transport: Transport,
    sender: Sender<Incoming>,
    incoming: Receiver<Incoming>,
    initialized: bool,
    pending: HashMap<i64, Pending>,
    queued: VecDeque<Queued>,
    tools: Vec<Tool>,
    resources: Vec<Resource>,
    prompts: Vec<Prompt>,
    resource_pages: usize,
    prompt_pages: usize,
    tools_loaded: bool,
    logs: VecDeque<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Transport::Stdio { child, .. } = &mut self.transport {
            let _ = child.kill();
            let _ = child.wait();
        }
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

    pub fn list_resources(&mut self, server: &str) -> Result<(), String> {
        if let Some(state) = self.servers.get_mut(server) {
            state.resources.clear();
            state.resource_pages = 0;
        }
        self.request(
            server,
            "resources/list",
            json!({}),
            ReplyKind::ResourcesList,
            None,
        )
    }

    fn list_resources_page(&mut self, server: &str, cursor: &str) -> Result<(), String> {
        self.request(
            server,
            "resources/list",
            json!({"cursor": cursor}),
            ReplyKind::ResourcesList,
            None,
        )
    }

    pub fn list_resource_templates(&mut self, server: &str) -> Result<(), String> {
        self.request(
            server,
            "resources/templates/list",
            json!({}),
            ReplyKind::ResourceTemplatesList,
            None,
        )
    }

    pub fn read_resource(&mut self, server: &str, uri: &str) -> Result<(), String> {
        self.request(
            server,
            "resources/read",
            json!({"uri": uri}),
            ReplyKind::ResourceRead,
            Some(uri.to_string()),
        )
    }

    pub fn list_prompts(&mut self, server: &str) -> Result<(), String> {
        if let Some(state) = self.servers.get_mut(server) {
            state.prompts.clear();
            state.prompt_pages = 0;
        }
        self.request(
            server,
            "prompts/list",
            json!({}),
            ReplyKind::PromptsList,
            None,
        )
    }

    fn list_prompts_page(&mut self, server: &str, cursor: &str) -> Result<(), String> {
        self.request(
            server,
            "prompts/list",
            json!({"cursor": cursor}),
            ReplyKind::PromptsList,
            None,
        )
    }

    pub fn get_prompt(&mut self, server: &str, name: &str) -> Result<(), String> {
        self.request(
            server,
            "prompts/get",
            json!({"name": name}),
            ReplyKind::PromptGet,
            Some(name.to_string()),
        )
    }

    pub fn respond(&mut self, server: &str, id: i64, result: Value) -> Result<(), String> {
        self.respond_value(
            server,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        )
    }

    pub fn respond_error(&mut self, server: &str, id: i64, message: &str) -> Result<(), String> {
        self.respond_value(
            server,
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": message}}),
        )
    }

    fn respond_value(&mut self, server: &str, value: Value) -> Result<(), String> {
        let Some(server) = self.servers.get_mut(server) else {
            return Err("MCP server is not running".to_string());
        };
        match &mut server.transport {
            Transport::Stdio { writer, .. } => write_message(writer, &value),
            Transport::Http { .. } => Err(
                "HTTP server-initiated requests require an SSE stream and are unsupported"
                    .to_string(),
            ),
        }
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
            let write = send_request(server, id, method, params);
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
        let mut server = match start_transport(name, &config, &self.root) {
            Ok(server) => server,
            Err(error) => {
                self.failed.insert(name.to_string());
                return Err(error);
            }
        };
        let id = self.next_id();
        if let Err(error) = send_request(
            &mut server,
            id,
            "initialize",
            initialize_params(config.allow_sampling && config.url.is_none()),
        ) {
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

    fn handle_reply(
        &mut self,
        name: &str,
        pending: Pending,
        result: &Value,
        events: &mut Vec<Event>,
    ) {
        match pending.kind {
            ReplyKind::Initialize => self.finish_initialize(name, events),
            ReplyKind::ToolsList => {
                let tools = parse_tools(result);
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
                let (content, is_error) = parse_tool_result(result);
                events.push(Event::ToolResult {
                    server: name.to_string(),
                    tool: pending.tool.unwrap_or_default(),
                    content,
                    is_error,
                });
            }
            ReplyKind::ResourcesList => self.handle_resource_reply(name, None, result, events),
            ReplyKind::ResourceRead => {
                self.handle_resource_reply(name, pending.tool, result, events)
            }
            ReplyKind::ResourceTemplatesList => {
                events.push(Event::ResourceTemplates {
                    server: name.to_string(),
                    templates: parse_resource_templates(result),
                });
            }
            ReplyKind::PromptsList => self.handle_prompt_reply(name, None, result, events),
            ReplyKind::PromptGet => self.handle_prompt_reply(name, pending.tool, result, events),
        }
    }

    fn handle_resource_reply(
        &mut self,
        name: &str,
        uri: Option<String>,
        result: &Value,
        events: &mut Vec<Event>,
    ) {
        if let Some(uri) = uri {
            let (content, mime_type) = parse_resource_content(result);
            events.push(Event::Resource {
                server: name.to_string(),
                uri,
                content,
                mime_type,
            });
        } else {
            let resources = parse_resources(result);
            let next = next_cursor(result);
            let complete = {
                let Some(server) = self.servers.get_mut(name) else {
                    return;
                };
                server.resources.extend(resources);
                server.resource_pages += 1;
                next.is_none() || server.resource_pages >= 100
            };
            if !complete {
                if let Some(cursor) = next {
                    let _ = self.list_resources_page(name, &cursor);
                }
                return;
            }
            let resources = self
                .servers
                .get(name)
                .map(|server| server.resources.clone())
                .unwrap_or_default();
            events.push(Event::Resources {
                server: name.to_string(),
                resources,
            });
        }
    }

    fn handle_prompt_reply(
        &mut self,
        name: &str,
        prompt: Option<String>,
        result: &Value,
        events: &mut Vec<Event>,
    ) {
        if let Some(prompt) = prompt {
            events.push(Event::Prompt {
                server: name.to_string(),
                name: prompt,
                content: parse_prompt_content(result),
            });
        } else {
            let prompts = parse_prompts(result);
            let next = next_cursor(result);
            let complete = {
                let Some(server) = self.servers.get_mut(name) else {
                    return;
                };
                server.prompts.extend(prompts);
                server.prompt_pages += 1;
                next.is_none() || server.prompt_pages >= 100
            };
            if !complete {
                if let Some(cursor) = next {
                    let _ = self.list_prompts_page(name, &cursor);
                }
                return;
            }
            let prompts = self
                .servers
                .get(name)
                .map(|server| server.prompts.clone())
                .unwrap_or_default();
            events.push(Event::Prompts {
                server: name.to_string(),
                prompts,
            });
        }
    }

    fn handle_server_request(
        &mut self,
        name: &str,
        id: i64,
        method: &str,
        value: &Value,
        events: &mut Vec<Event>,
    ) {
        if method == "sampling/createMessage" {
            let allowed = self
                .configs
                .get(name)
                .is_some_and(|config| config.allow_sampling && config.url.is_none());
            if allowed {
                events.push(Event::Sampling {
                    server: name.to_string(),
                    id,
                    params: value.get("params").cloned().unwrap_or(Value::Null),
                });
            } else {
                let _ = self.respond_error(name, id, "sampling is disabled");
            }
            return;
        }
        let _ = self.respond_error(name, id, "unsupported MCP client method");
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
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                self.handle_server_request(name, id, method, &value, events);
                return;
            }
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
            self.handle_reply(name, pending, &result, events);
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
            if let Err(error) = send_notification(server, "notifications/initialized") {
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

fn start_transport(name: &str, config: &McpServerConfig, root: &Path) -> Result<Server, String> {
    if let Some(url) = config.url.as_deref().filter(|url| !url.trim().is_empty()) {
        return Ok(spawn_http_server(url));
    }
    spawn_server(name, config, root)
}

fn spawn_http_server(url: &str) -> Server {
    let (sender, incoming) = mpsc::channel();
    Server {
        transport: Transport::Http {
            url: url.to_string(),
            session: Arc::new(Mutex::new(None)),
        },
        sender,
        incoming,
        initialized: false,
        pending: HashMap::new(),
        queued: VecDeque::new(),
        tools: Vec::new(),
        resources: Vec::new(),
        prompts: Vec::new(),
        resource_pages: 0,
        prompt_pages: 0,
        tools_loaded: false,
        logs: VecDeque::new(),
    }
}

fn send_request(
    server: &mut Server,
    id: i64,
    method: &'static str,
    params: Value,
) -> Result<(), String> {
    let sender = server.sender.clone();
    match &mut server.transport {
        Transport::Stdio { writer, .. } => write_request(writer, id, method, params),
        Transport::Http { url, session } => {
            spawn_http_request(
                url.clone(),
                Arc::clone(session),
                sender,
                Some(id),
                method,
                params,
            );
            Ok(())
        }
    }
}

fn send_notification(server: &mut Server, method: &'static str) -> Result<(), String> {
    let sender = server.sender.clone();
    match &mut server.transport {
        Transport::Stdio { writer, .. } => write_notification(writer, method),
        Transport::Http { url, session } => {
            spawn_http_request(
                url.clone(),
                Arc::clone(session),
                sender,
                None,
                method,
                json!({}),
            );
            Ok(())
        }
    }
}

fn spawn_http_request(
    url: String,
    session: Arc<Mutex<Option<String>>>,
    sender: Sender<Incoming>,
    id: Option<i64>,
    method: &'static str,
    params: Value,
) {
    std::thread::spawn(move || {
        let body = if let Some(id) = id {
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        } else {
            json!({"jsonrpc": "2.0", "method": method, "params": params})
        };
        let body_file = match tempfile::NamedTempFile::new().and_then(|mut file| {
            file.write_all(body.to_string().as_bytes())?;
            Ok(file)
        }) {
            Ok(file) => file,
            Err(error) => {
                let _ = sender.send(Incoming::ProtocolError(format!("HTTP request: {error}")));
                return;
            }
        };
        let mut config = format!("url = {}\nrequest = \"POST\"\n", quote_curl(&url));
        config.push_str("header = \"Content-Type: application/json\"\n");
        config.push_str("header = \"Accept: application/json, text/event-stream\"\n");
        if let Ok(session) = session.lock()
            && let Some(session) = session.as_deref()
        {
            config.push_str(&format!(
                "header = {}\n",
                quote_curl(&format!("Mcp-Session-Id: {session}"))
            ));
        }
        config.push_str(&format!(
            "data-binary = {}\n",
            quote_curl(&format!("@{}", body_file.path().to_string_lossy()))
        ));
        config.push_str("max-time = 60\n");
        let mut child = match Command::new("curl")
            .args(["--config", "-", "--silent", "--show-error", "--include"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = sender.send(Incoming::ProtocolError(format!("HTTP curl: {error}")));
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take()
            && let Err(error) = stdin.write_all(config.as_bytes())
        {
            let _ = child.kill();
            let _ = child.wait();
            let _ = sender.send(Incoming::ProtocolError(format!("HTTP config: {error}")));
            return;
        }
        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => {
                let _ = sender.send(Incoming::ProtocolError(format!("HTTP curl: {error}")));
                return;
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = sender.send(Incoming::ProtocolError(format!(
                "HTTP request: {}",
                stderr.trim()
            )));
            return;
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        if let Err(error) = handle_http_response(&raw, id, &session, &sender) {
            let _ = sender.send(Incoming::ProtocolError(error));
        }
    });
}

fn handle_http_response(
    raw: &str,
    id: Option<i64>,
    session: &Mutex<Option<String>>,
    sender: &Sender<Incoming>,
) -> Result<(), String> {
    let (headers, body) = raw.split_once("\r\n\r\n").unwrap_or(("", raw));
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    if !(200..300).contains(&status) {
        if let Some(id) = id {
            let _ = sender.send(Incoming::Line(
                json!({"jsonrpc": "2.0", "id": id, "error": {"message": body.trim()}}).to_string(),
            ));
        }
        return Ok(());
    }
    for line in headers.lines() {
        if let Some(value) = line.strip_prefix("Mcp-Session-Id:")
            && let Ok(mut session) = session.lock()
        {
            *session = Some(value.trim().to_string());
        }
    }
    if id.is_none() {
        return Ok(());
    }
    let mut saw_json = false;
    for line in body.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.starts_with('{') {
                let _ = sender.send(Incoming::Line(data.to_string()));
            }
        } else if line.starts_with('{') {
            saw_json = true;
            let _ = sender.send(Incoming::Line(line.to_string()));
        }
    }
    if !saw_json && body.trim().starts_with('{') {
        let _ = sender.send(Incoming::Line(body.trim().to_string()));
    }
    Ok(())
}

fn quote_curl(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
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
    let stderr_tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if stderr_tx.send(Incoming::Stderr(line)).is_err() {
                break;
            }
        }
    });
    Ok(Server {
        transport: Transport::Stdio { child, writer },
        sender: tx,
        incoming,
        initialized: false,
        pending: HashMap::new(),
        queued: VecDeque::new(),
        tools: Vec::new(),
        resources: Vec::new(),
        prompts: Vec::new(),
        resource_pages: 0,
        prompt_pages: 0,
        tools_loaded: false,
        logs: VecDeque::new(),
    })
}

fn initialize_params(allow_sampling: bool) -> Value {
    let capabilities = if allow_sampling {
        json!({"sampling": {}})
    } else {
        json!({})
    };
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": capabilities,
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

fn next_cursor(result: &Value) -> Option<String> {
    result
        .get("nextCursor")
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_string)
}

fn parse_resource_templates(result: &Value) -> Vec<ResourceTemplate> {
    result
        .get("resourceTemplates")
        .and_then(Value::as_array)
        .map(|templates| {
            templates
                .iter()
                .filter_map(|template| {
                    let uri_template = template.get("uriTemplate")?.as_str()?.to_string();
                    Some(ResourceTemplate {
                        uri_template,
                        name: template
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: template
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        mime_type: template
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_resources(result: &Value) -> Vec<Resource> {
    result
        .get("resources")
        .and_then(Value::as_array)
        .map(|resources| {
            resources
                .iter()
                .filter_map(|resource| {
                    let uri = resource.get("uri")?.as_str()?.to_string();
                    Some(Resource {
                        uri,
                        name: resource
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: resource
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        mime_type: resource
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_resource_content(result: &Value) -> (String, String) {
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut text = String::new();
    let mut mime_type = String::new();
    for item in contents {
        if let Some(value) = item.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(value);
        } else if let Some(blob) = item.get("blob").and_then(Value::as_str) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("[binary resource] ");
            text.push_str(blob);
        }
        if mime_type.is_empty()
            && let Some(value) = item.get("mimeType").and_then(Value::as_str)
        {
            mime_type = value.to_string();
        }
    }
    (text, mime_type)
}

fn parse_prompts(result: &Value) -> Vec<Prompt> {
    result
        .get("prompts")
        .and_then(Value::as_array)
        .map(|prompts| {
            prompts
                .iter()
                .filter_map(|prompt| {
                    let name = prompt.get("name")?.as_str()?.to_string();
                    Some(Prompt {
                        name,
                        description: prompt
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: prompt
                            .get("arguments")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| {
                                        item.get("name").and_then(Value::as_str).map(str::to_string)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_prompt_content(result: &Value) -> String {
    result
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let role = message
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("message");
                    let content = message
                        .pointer("/content/text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    format!("{role}: {content}")
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
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
                        read_only: tool
                            .pointer("/annotations/readOnlyHint")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        destructive: tool
                            .pointer("/annotations/destructiveHint")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        open_world: tool
                            .pointer("/annotations/openWorldHint")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
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
            url: None,
            allow_sampling: false,
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
    fn sampling_request_is_emitted_and_response_is_written_back() {
        if !python_available() {
            return;
        }
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("mock_mcp.py");
        let config = McpServerConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: None,
            env: BTreeMap::from([("MOCK_MCP_MODE".to_string(), "sampling".to_string())]),
            url: None,
            allow_sampling: true,
        };
        let mut manager = McpManager::new(
            BTreeMap::from([("mock".to_string(), config)]),
            std::env::temp_dir(),
        );
        manager.list_tools("mock").expect("start");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::Sampling { .. })
        });
        let (id, params) = events
            .iter()
            .find_map(|event| match event {
                Event::Sampling { id, params, .. } => Some((*id, params.clone())),
                _ => None,
            })
            .expect("sampling event");
        assert_eq!(id, 900);
        assert!(params.to_string().contains("sample me"));
        manager
            .respond(
                "mock",
                id,
                json!({"role": "assistant", "content": {"type": "text", "text": "sample answer"}}),
            )
            .expect("respond");
        let events = wait_for(
            &mut manager,
            |event| matches!(event, Event::Log { line, .. } if line.contains("sampling response")),
        );
        assert!(!events.is_empty());
    }

    #[test]
    fn paginated_resources_and_prompts_are_aggregated() {
        if !python_available() {
            return;
        }
        let mut manager = mock("paginated");
        manager.list_resources("mock").expect("list resources");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::Resources { .. })
        });
        let resources = events
            .iter()
            .find_map(|event| match event {
                Event::Resources { resources, .. } => Some(resources.clone()),
                _ => None,
            })
            .expect("resources");
        assert_eq!(resources.len(), 2);
        manager.list_prompts("mock").expect("list prompts");
        let events = wait_for(&mut manager, |event| matches!(event, Event::Prompts { .. }));
        let prompts = events
            .iter()
            .find_map(|event| match event {
                Event::Prompts { prompts, .. } => Some(prompts.clone()),
                _ => None,
            })
            .expect("prompts");
        assert_eq!(prompts.len(), 2);
    }

    #[test]
    fn resources_and_prompts_round_trip_over_stdio() {
        if !python_available() {
            return;
        }
        let mut manager = mock("normal");
        manager.list_resources("mock").expect("list resources");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::Resources { .. })
        });
        let resources = events
            .iter()
            .find_map(|event| match event {
                Event::Resources { resources, .. } => Some(resources.clone()),
                _ => None,
            })
            .expect("resources");
        assert_eq!(resources[0].uri, "mock://readme");
        manager
            .read_resource("mock", "mock://readme")
            .expect("read");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::Resource { .. })
        });
        assert!(events.iter().any(
            |event| matches!(event, Event::Resource { content, .. } if content == "hello resource")
        ));
        manager.list_prompts("mock").expect("list prompts");
        let events = wait_for(&mut manager, |event| matches!(event, Event::Prompts { .. }));
        assert!(events.iter().any(
            |event| matches!(event, Event::Prompts { prompts, .. } if prompts[0].name == "review")
        ));
        manager.get_prompt("mock", "review").expect("get prompt");
        let events = wait_for(&mut manager, |event| matches!(event, Event::Prompt { .. }));
        assert!(events.iter().any(
            |event| matches!(event, Event::Prompt { content, .. } if content.contains("review it"))
        ));
    }

    #[test]
    fn http_transport_initializes_lists_and_calls() {
        if !python_available() {
            return;
        }
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut request = [0u8; 8192];
                let read = stream.read(&mut request).expect("read");
                let text = String::from_utf8_lossy(&request[..read]);
                let body = text
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body)
                    .unwrap_or("");
                let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                let method = value.get("method").and_then(Value::as_str).unwrap_or("");
                let (status, session, payload) = match method {
                    "initialize" => (
                        "200 OK",
                        Some("Mcp-Session-Id: test-session\r\n"),
                        json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"http","version":"1"}}}),
                    ),
                    "tools/list" => (
                        "200 OK",
                        None,
                        json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}}),
                    ),
                    "tools/call" => (
                        "200 OK",
                        None,
                        json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"pong"}],"isError":false}}),
                    ),
                    _ => ("202 Accepted", None, Value::Null),
                };
                let body = if payload.is_null() {
                    String::new()
                } else {
                    payload.to_string()
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\n{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    session.unwrap_or(""),
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        let mut manager = McpManager::new(
            BTreeMap::from([(
                "http".to_string(),
                McpServerConfig {
                    command: String::new(),
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    url: Some(format!("http://{address}/mcp")),
                    allow_sampling: false,
                },
            )]),
            std::env::temp_dir(),
        );
        manager.list_tools("http").expect("list tools");
        let events = wait_for(&mut manager, |event| matches!(event, Event::Tools { .. }));
        let tools = events
            .iter()
            .find_map(|event| match event {
                Event::Tools { tools, .. } => Some(tools.clone()),
                _ => None,
            })
            .expect("tools");
        assert_eq!(tools[0].name, "echo");
        manager
            .call_tool("http", "echo", json!({"text":"ping"}))
            .expect("call");
        let events = wait_for(&mut manager, |event| {
            matches!(event, Event::ToolResult { .. })
        });
        assert!(events.iter().any(|event| matches!(event, Event::ToolResult { content, .. } if content.iter().any(|item| matches!(item, ToolContent::Text(text) if text == "pong")))));
        server.join().expect("server");
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
    fn parses_tool_capability_annotations() {
        let tools = parse_tools(&json!({
            "tools": [
                {
                    "name": "reader",
                    "description": "",
                    "inputSchema": {},
                    "annotations": {"readOnlyHint": true, "destructiveHint": false}
                },
                {
                    "name": "writer",
                    "description": "",
                    "inputSchema": {},
                    "annotations": {"destructiveHint": false}
                },
                {
                    "name": "unannotated",
                    "description": "",
                    "inputSchema": {}
                }
            ]
        }));
        assert_eq!(tools[0].capability(), "read-only");
        assert_eq!(tools[1].capability(), "write");
        assert_eq!(tools[2].capability(), "destructive");
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
            read_only: false,
            destructive: true,
            open_world: false,
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
            url: None,
            allow_sampling: false,
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
