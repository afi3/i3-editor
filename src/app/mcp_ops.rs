//! MCP menu, trust prompt, tool calls, and result overlay.

use serde_json::Value;

use crate::input::{KeyCombo, PromptKind, PromptState};
use crate::mcp::ToolContent;
use crate::state::{HelpView, InputMode};

use super::App;

impl App {
    pub(crate) fn open_mcp(&mut self) {
        let mut prompt = PromptState::new(PromptKind::Mcp);
        self.fill_mcp_servers(&mut prompt);
        self.state.mode = InputMode::Prompt(prompt);
        self.state.help = None;
        self.state.completions = None;
        self.state.hover = None;
        self.dirty = true;
    }

    pub(crate) fn open_mcp_add(&mut self) {
        let mut prompt = PromptState::new(PromptKind::McpAdd);
        prompt.set_candidates(
            MCP_TEMPLATES
                .iter()
                .map(|template| format!("{} — {}", template.id, template.description))
                .collect(),
        );
        self.state.mode = InputMode::Prompt(prompt);
        self.dirty = true;
    }

    pub(crate) fn submit_mcp_add(&mut self, prompt: PromptState) {
        let Some(candidate) = prompt.selected_candidate() else {
            return;
        };
        let Some(id) = candidate.split(" — ").next() else {
            return;
        };
        let Some(template) = MCP_TEMPLATES.iter().find(|template| template.id == id) else {
            return;
        };
        if template.needs_path {
            let mut path = PromptState::new(PromptKind::McpAddPath);
            path.mcp_template = Some(template.id.to_string());
            path.set_input(self.state.explorer.root.to_string_lossy());
            self.state.mode = InputMode::Prompt(path);
        } else {
            self.add_mcp_template(template, None);
        }
    }

    pub(crate) fn submit_mcp_add_path(&mut self, prompt: PromptState) {
        let Some(template_id) = prompt.mcp_template.clone() else {
            return;
        };
        let Some(template) = MCP_TEMPLATES
            .iter()
            .find(|template| template.id == template_id)
        else {
            return;
        };
        let raw = prompt.fields.first().cloned().unwrap_or_default();
        let path = if std::path::Path::new(&raw).is_absolute() {
            std::path::PathBuf::from(raw)
        } else {
            self.state.explorer.root.join(raw)
        };
        if !path.is_dir() {
            let mut retry = PromptState::new(PromptKind::McpAddPath);
            retry.mcp_template = Some(template_id);
            retry.set_input(path.to_string_lossy());
            self.state.mode = InputMode::Prompt(retry);
            self.notify(format!("not a directory: {}", path.display()));
            return;
        }
        self.add_mcp_template(template, Some(path));
    }

    fn add_mcp_template(&mut self, template: &McpTemplate, path: Option<std::path::PathBuf>) {
        let mut name = template.id.to_string();
        let mut suffix = 2;
        while self.state.config.mcp.servers.contains_key(&name) {
            name = format!("{}-{suffix}", template.id);
            suffix += 1;
        }
        let mut args: Vec<String> = template.args.iter().map(|arg| (*arg).to_string()).collect();
        if let Some(path) = path {
            args.push(path.to_string_lossy().into_owned());
        }
        let config = crate::mcp::McpServerConfig {
            command: template.command.to_string(),
            args,
            cwd: None,
            env: Default::default(),
        };
        self.state
            .config
            .mcp
            .servers
            .insert(name.clone(), config.clone());
        if self.persist_mcp_state
            && let Err(error) = crate::config::persist_mcp_server(&name, &config)
        {
            self.notify(format!("MCP {name} added for this session only: {error}"));
        }
        if let Some(manager) = self.mcp.as_mut() {
            manager.restart(
                self.state.config.mcp.servers.clone(),
                self.state.explorer.root.clone(),
            );
        }
        self.state.mode = InputMode::Normal;
        self.start_mcp_flow(&name);
    }

    pub(crate) fn handle_mcp_key(&mut self, combo: KeyCombo) -> bool {
        let Some(view) = self.state.mcp_view.as_mut() else {
            return false;
        };
        if super::help::read_only_key(view, combo) {
            self.state.mcp_view = None;
        }
        true
    }

    pub(crate) fn pump_mcp(&mut self) {
        let Some(manager) = self.mcp.as_mut() else {
            return;
        };
        let events = manager.pump();
        if events.is_empty() {
            return;
        }
        let mut refresh_tools = false;
        for event in events {
            match event {
                crate::mcp::Event::Warning(message) => {
                    if self.state.ai.waiting.is_some() {
                        self.state.ai.waiting = None;
                        self.state.ai.busy = false;
                        self.state.ai.entries.push(format!("[error] {message}"));
                    }
                    self.notify(message);
                }
                crate::mcp::Event::Tools { .. } => refresh_tools = true,
                crate::mcp::Event::ToolResult {
                    server,
                    tool,
                    content,
                    is_error,
                } => {
                    if self.state.ai.waiting.is_some() {
                        self.finish_ai_tool(&server, &tool, &content, is_error);
                    } else {
                        self.show_mcp_result(&server, &tool, &content, is_error);
                    }
                }
                crate::mcp::Event::Log { .. } => {}
            }
        }
        self.dirty = true;
        if refresh_tools {
            let server = match &self.state.mode {
                InputMode::Prompt(prompt) if prompt.kind == PromptKind::McpTools => {
                    prompt.mcp_server.clone()
                }
                _ => None,
            };
            if let Some(server) = server {
                let mut prompt = PromptState::new(PromptKind::McpTools);
                prompt.mcp_server = Some(server.clone());
                self.fill_mcp_tools(&mut prompt);
                self.state.mode = InputMode::Prompt(prompt);
            }
        }
    }

    fn fill_mcp_servers(&self, prompt: &mut PromptState) {
        let mut candidates = Vec::new();
        for name in self.state.config.mcp.servers.keys() {
            let status = if self.mcp_trusted(name) {
                "trusted"
            } else {
                "trust required"
            };
            candidates.push(format!("{name} — {status}"));
        }
        if candidates.is_empty() {
            candidates.push("No MCP servers configured".to_string());
        }
        prompt.set_candidates(candidates);
    }

    fn fill_mcp_tools(&self, prompt: &mut PromptState) {
        let server = prompt.mcp_server.clone().unwrap_or_default();
        let candidates = self
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.tools(&server))
            .map(|tools| {
                tools
                    .iter()
                    .map(|tool| format!("{} — {}", tool.name, tool.description))
                    .collect()
            })
            .unwrap_or_default();
        prompt.set_candidates(candidates);
    }

    pub(crate) fn mcp_trusted(&self, server: &str) -> bool {
        self.mcp
            .as_ref()
            .and_then(|mcp| mcp.config_hash(server))
            .is_some_and(|hash| self.mcp_trusted.contains(&hash))
    }

    pub(crate) fn submit_mcp_prompt(&mut self, prompt: PromptState) {
        match prompt.kind {
            PromptKind::Mcp => self.submit_mcp_server(prompt),
            PromptKind::McpTrust => self.submit_mcp_trust(prompt),
            PromptKind::McpTools => self.submit_mcp_tools(prompt),
            PromptKind::McpArgs => self.submit_mcp_args(prompt),
            PromptKind::McpConfirm => self.submit_mcp_confirm(prompt),
            PromptKind::McpAdd => self.submit_mcp_add(prompt),
            PromptKind::McpAddPath => self.submit_mcp_add_path(prompt),
            _ => {}
        }
    }

    pub(crate) fn submit_mcp_server(&mut self, prompt: PromptState) {
        let Some(candidate) = prompt.selected_candidate() else {
            return;
        };
        let Some(name) = candidate
            .split(" — ")
            .next()
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        self.start_mcp_flow(name);
    }

    pub(crate) fn submit_mcp_trust(&mut self, prompt: PromptState) {
        let trusted = prompt
            .selected_candidate()
            .is_some_and(|candidate| candidate.starts_with("Trust and start"));
        let Some(server) = prompt.mcp_server else {
            return;
        };
        if !trusted {
            self.notify("MCP server start cancelled");
            return;
        }
        let Some(hash) = self.mcp.as_ref().and_then(|mcp| mcp.config_hash(&server)) else {
            self.notify(format!("MCP {server}: configuration disappeared"));
            return;
        };
        self.mcp_trusted.insert(hash);
        if let Err(error) = crate::config::save_mcp_trust(&self.mcp_trusted) {
            self.notify(format!(
                "MCP trust applied for this session, not saved: {error}"
            ));
        }
        self.launch_mcp(&server);
    }

    pub(crate) fn submit_mcp_tools(&mut self, prompt: PromptState) {
        let Some(candidate) = prompt.selected_candidate() else {
            return;
        };
        let Some(tool) = candidate
            .split(" — ")
            .next()
            .filter(|tool| !tool.is_empty())
        else {
            return;
        };
        let tool = tool.to_string();
        let Some(server) = prompt.mcp_server else {
            return;
        };
        if tool == "logs" {
            self.show_mcp_logs(&server);
            return;
        }
        self.record_mcp_recent(&server, &tool);
        let Some(tool_def) = self
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.tools(&server))
            .and_then(|tools| tools.iter().find(|candidate| candidate.name == tool))
            .cloned()
        else {
            self.notify(format!("MCP {server}: tool {tool} disappeared"));
            return;
        };
        let fields = tool_def.arguments();
        let values: Vec<String> = fields
            .iter()
            .map(|field| self.prefill_mcp_arg(&tool_def.name, field))
            .collect();
        let mut args = PromptState::new(PromptKind::McpArgs);
        args.mcp_server = Some(server);
        args.mcp_tool = Some(tool);
        args.mcp_fields = fields;
        args.fields = values;
        self.state.mode = InputMode::Prompt(args);
    }

    pub(crate) fn submit_mcp_args(&mut self, prompt: PromptState) {
        let Some(server) = prompt.mcp_server.clone() else {
            return;
        };
        let Some(tool) = prompt.mcp_tool.clone() else {
            return;
        };
        let value = match build_mcp_arguments(
            &prompt.mcp_fields,
            &prompt.fields,
            &self.state.explorer.root,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.notify(error);
                let mut retry = PromptState::new(PromptKind::McpArgs);
                retry.mcp_server = Some(server);
                retry.mcp_tool = Some(tool);
                retry.mcp_fields = prompt.mcp_fields;
                retry.fields = prompt.fields;
                self.state.mode = InputMode::Prompt(retry);
                return;
            }
        };
        let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
        let mut confirm = PromptState::new(PromptKind::McpConfirm);
        confirm.mcp_server = Some(server);
        confirm.mcp_tool = Some(tool);
        confirm.mcp_arguments = Some(text);
        confirm.set_candidates(vec!["Call tool".to_string(), "Cancel".to_string()]);
        self.state.mode = InputMode::Prompt(confirm);
    }

    pub(crate) fn submit_mcp_confirm(&mut self, prompt: PromptState) {
        if prompt.selected_candidate() != Some("Call tool") {
            self.notify("MCP tool call cancelled");
            return;
        }
        let (Some(server), Some(tool)) = (prompt.mcp_server, prompt.mcp_tool) else {
            return;
        };
        let text = prompt.mcp_arguments.clone().unwrap_or_default();
        let Ok(arguments) = serde_json::from_str::<Value>(&text) else {
            self.notify("invalid JSON arguments");
            return;
        };
        let Some(manager) = self.mcp.as_mut() else {
            self.notify("MCP manager unavailable");
            return;
        };
        if let Err(error) = manager.call_tool(&server, &tool, arguments) {
            self.notify(error);
        } else {
            self.notify(format!("calling {server}/{tool}"));
        }
    }

    fn record_mcp_recent(&mut self, server: &str, tool: &str) {
        let entry = self.mcp_recent.entry(server.to_string()).or_default();
        entry.retain(|name| name != tool);
        entry.insert(0, tool.to_string());
        entry.truncate(6);
        if self.persist_mcp_state
            && let Err(error) = crate::config::save_mcp_recent(&self.mcp_recent)
        {
            self.notify(format!("MCP recent tools not saved: {error}"));
        }
        self.dirty = true;
    }

    pub(crate) fn prefill_mcp_arg(&self, tool: &str, arg: &crate::mcp::ToolArg) -> String {
        use crate::mcp::ArgKind;
        if matches!(arg.kind, ArgKind::String)
            && (arg.name == "path" || arg.name.ends_with("_path") || arg.name == "file")
        {
            let root = &self.state.explorer.root;
            let active = self
                .state
                .active_document()
                .and_then(|document| document.path.clone());
            let path = if tool.starts_with("read") || tool.contains("info") {
                active.unwrap_or_else(|| root.clone())
            } else {
                root.clone()
            };
            return path.to_string_lossy().into_owned();
        }
        match &arg.kind {
            ArgKind::Enum(values) => values.first().cloned().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn start_mcp_flow(&mut self, server: &str) {
        let Some(hash) = self.mcp.as_ref().and_then(|mcp| mcp.config_hash(server)) else {
            self.notify(format!("MCP {server}: configuration disappeared"));
            return;
        };
        if self.mcp_trusted.contains(&hash) {
            self.launch_mcp(server);
            return;
        }
        let Some(config) = self.state.config.mcp.servers.get(server) else {
            return;
        };
        let mut command = config.command.clone();
        if !config.args.is_empty() {
            command.push(' ');
            command.push_str(&config.args.join(" "));
        }
        let mut prompt = PromptState::new(PromptKind::McpTrust);
        prompt.mcp_server = Some(server.to_string());
        prompt.set_candidates(vec![
            format!("Trust and start — {command}"),
            "Cancel".to_string(),
        ]);
        self.state.mode = InputMode::Prompt(prompt);
    }

    fn launch_mcp(&mut self, server: &str) {
        let Some(manager) = self.mcp.as_mut() else {
            self.notify("MCP manager unavailable");
            return;
        };
        if let Err(error) = manager.list_tools(server) {
            self.notify(error);
            return;
        }
        let mut prompt = PromptState::new(PromptKind::McpTools);
        prompt.mcp_server = Some(server.to_string());
        self.fill_mcp_tools(&mut prompt);
        self.state.mode = InputMode::Prompt(prompt);
        self.notify(format!("starting MCP {server}"));
    }

    fn show_mcp_logs(&mut self, server: &str) {
        let lines = self
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.logs(server))
            .filter(|logs| !logs.is_empty())
            .map(|logs| {
                let mut lines = vec![format!("MCP logs: {server}"), String::new()];
                lines.extend(logs.iter().cloned());
                lines.push(String::new());
                lines.push("Esc or q closes this overlay".to_string());
                lines
            })
            .unwrap_or_else(|| vec![format!("MCP logs: {server}"), "no logs".to_string()]);
        self.state.mcp_view = Some(HelpView { lines, scroll: 0 });
        self.state.completions = None;
        self.state.hover = None;
    }

    fn show_mcp_result(
        &mut self,
        server: &str,
        tool: &str,
        content: &[ToolContent],
        is_error: bool,
    ) {
        let mut lines = vec![
            "Esc or q closes this overlay".to_string(),
            format!("MCP result: {server}/{tool}"),
        ];
        if is_error {
            lines.push("isError: true".to_string());
        }
        lines.push(String::new());
        for item in content {
            match item {
                ToolContent::Text(text) => lines.extend(text.lines().map(str::to_string)),
                ToolContent::Other(value) => {
                    lines.push(
                        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
                    );
                }
            }
        }
        self.state.mcp_view = Some(HelpView { lines, scroll: 0 });
        self.state.completions = None;
        self.state.hover = None;
    }
}

fn build_mcp_arguments(
    fields: &[crate::mcp::ToolArg],
    values: &[String],
    root: &std::path::Path,
) -> Result<Value, String> {
    use crate::mcp::ArgKind;
    let mut object = serde_json::Map::new();
    for (field, raw) in fields.iter().zip(values.iter()) {
        let raw = raw.trim();
        if raw.is_empty() {
            if field.required {
                return Err(format!("{} is required", field.name));
            }
            continue;
        }
        let value = match &field.kind {
            ArgKind::String | ArgKind::Enum(_) => {
                if (field.name == "path" || field.name.ends_with("_path") || field.name == "file")
                    && !std::path::Path::new(raw).is_absolute()
                {
                    Value::String(root.join(raw).to_string_lossy().into_owned())
                } else {
                    Value::String(raw.to_string())
                }
            }
            ArgKind::Integer => raw
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| format!("{} must be an integer", field.name))?,
            ArgKind::Number => raw
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| format!("{} must be a number", field.name))?,
            ArgKind::Boolean => match raw.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Value::Bool(true),
                "false" | "0" | "no" => Value::Bool(false),
                _ => return Err(format!("{} must be true or false", field.name)),
            },
            ArgKind::Json => serde_json::from_str(raw)
                .map_err(|error| format!("{} must be valid JSON: {error}", field.name))?,
        };
        object.insert(field.name.clone(), value);
    }
    Ok(Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{ArgKind, ToolArg};

    #[test]
    fn schema_arguments_build_typed_json_and_resolve_paths() {
        let fields = vec![
            ToolArg {
                name: "path".to_string(),
                description: String::new(),
                kind: ArgKind::String,
                required: true,
            },
            ToolArg {
                name: "line".to_string(),
                description: String::new(),
                kind: ArgKind::Integer,
                required: false,
            },
        ];
        let value = build_mcp_arguments(
            &fields,
            &["src/main.rs".to_string(), "42".to_string()],
            std::path::Path::new("/tmp/project"),
        )
        .expect("arguments");
        assert_eq!(value["path"], "/tmp/project/src/main.rs");
        assert_eq!(value["line"], 42);
        assert!(
            build_mcp_arguments(&fields, &["".to_string()], std::path::Path::new("/tmp")).is_err()
        );
    }
}

#[derive(Clone, Copy)]
struct McpTemplate {
    id: &'static str,
    description: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    needs_path: bool,
}

const MCP_TEMPLATES: &[McpTemplate] = &[
    McpTemplate {
        id: "filesystem",
        description: "read and list files under one directory",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem"],
        needs_path: true,
    },
    McpTemplate {
        id: "everything",
        description: "protocol demo server",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-everything"],
        needs_path: false,
    },
];
