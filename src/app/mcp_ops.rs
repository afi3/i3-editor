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
                crate::mcp::Event::Warning(message) => self.notify(message),
                crate::mcp::Event::Tools { .. } => refresh_tools = true,
                crate::mcp::Event::ToolResult {
                    server,
                    tool,
                    content,
                    is_error,
                } => self.show_mcp_result(&server, &tool, &content, is_error),
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

    fn mcp_trusted(&self, server: &str) -> bool {
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
        let mut args = PromptState::new(PromptKind::McpArgs);
        args.mcp_server = Some(server);
        args.mcp_tool = Some(tool);
        args.set_input("{}");
        self.state.mode = InputMode::Prompt(args);
    }

    pub(crate) fn submit_mcp_args(&mut self, prompt: PromptState) {
        let Some(server) = prompt.mcp_server.clone() else {
            return;
        };
        let Some(tool) = prompt.mcp_tool.clone() else {
            return;
        };
        let text = prompt.fields.first().cloned().unwrap_or_default();
        if let Err(error) = serde_json::from_str::<Value>(&text) {
            self.notify(format!("invalid JSON arguments: {error}"));
            let mut retry = PromptState::new(PromptKind::McpArgs);
            retry.mcp_server = Some(server);
            retry.mcp_tool = Some(tool);
            retry.set_input(text);
            self.state.mode = InputMode::Prompt(retry);
            return;
        }
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
