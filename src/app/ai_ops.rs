//! AI panel input, bounded MCP tool loop, and transcript.

use ratatui::crossterm::event::KeyCode;
use serde_json::{Value, json};

use crate::command::Command;
use crate::input::KeyCombo;
use crate::mcp::ToolContent;
use crate::state::{AiToolCall, AiWaiting};

use super::App;

const MAX_ROUNDS: usize = 8;

impl App {
    pub(crate) fn open_ai(&mut self) {
        self.state.ai.visible = true;
        self.state.ai.focused = true;
        if self.state.ai.entries.is_empty() {
            self.state
                .ai
                .entries
                .push("AI assistant. Type a question below.".to_string());
        }
        self.prime_ai_tools();
        self.dirty = true;
    }

    pub(crate) fn toggle_ai_panel(&mut self) {
        if self.state.ai.visible {
            self.state.ai.visible = false;
            self.state.ai.focused = false;
        } else {
            self.open_ai();
        }
        self.dirty = true;
    }

    pub(crate) fn handle_ai_key(&mut self, combo: KeyCombo) -> bool {
        if !self.state.ai.visible || !self.state.ai.focused {
            return false;
        }
        if self.handle_ai_global(combo) || self.handle_ai_interrupt(combo) {
            return true;
        }
        if self.state.ai.waiting.is_some() {
            return true;
        }
        if !self.state.ai.pending.is_empty() {
            self.handle_ai_confirmation(combo);
            return true;
        }
        self.handle_ai_edit(combo);
        self.dirty = true;
        true
    }

    fn handle_ai_global(&mut self, combo: KeyCombo) -> bool {
        let Some(command) = self.state.keymap.global.get(&combo).cloned() else {
            return false;
        };
        if !matches!(
            command,
            Command::Palette | Command::AiPanel | Command::AiChat | Command::Quit
        ) {
            return false;
        }
        let result = self.dispatch(command);
        self.finish(result);
        true
    }

    fn handle_ai_interrupt(&mut self, combo: KeyCombo) -> bool {
        if combo.code != KeyCode::Esc || (self.ai_task.is_none() && self.state.ai.waiting.is_none())
        {
            return false;
        }
        self.cancel_ai_turn();
        true
    }

    fn handle_ai_confirmation(&mut self, combo: KeyCombo) {
        match combo.code {
            KeyCode::Char('y') | KeyCode::Enter => self.allow_ai_tool(),
            KeyCode::Char('n') | KeyCode::Esc => self.deny_ai_tool(),
            _ => {}
        }
    }

    fn handle_ai_edit(&mut self, combo: KeyCombo) {
        let last = self.state.ai.entries.len().saturating_sub(1);
        match combo.code {
            KeyCode::Up => self.state.ai.scroll = self.state.ai.scroll.saturating_sub(1),
            KeyCode::Down => self.state.ai.scroll = (self.state.ai.scroll + 1).min(last),
            KeyCode::PageUp => self.state.ai.scroll = self.state.ai.scroll.saturating_sub(10),
            KeyCode::PageDown => self.state.ai.scroll = (self.state.ai.scroll + 10).min(last),
            KeyCode::Home => self.state.ai.scroll = 0,
            KeyCode::End => self.state.ai.scroll = last,
            _ => self.handle_ai_input(combo),
        }
    }

    fn handle_ai_input(&mut self, combo: KeyCombo) {
        match combo.code {
            KeyCode::Esc => {
                self.state.ai.focused = false;
            }
            KeyCode::Enter => self.submit_panel_input(),
            KeyCode::Backspace => {
                if self.state.ai.cursor > 0 {
                    let byte = char_byte(&self.state.ai.input, self.state.ai.cursor - 1);
                    let next = char_byte(&self.state.ai.input, self.state.ai.cursor);
                    self.state.ai.input.replace_range(byte..next, "");
                    self.state.ai.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let byte = char_byte(&self.state.ai.input, self.state.ai.cursor);
                let next = char_byte(&self.state.ai.input, self.state.ai.cursor + 1);
                if byte < next {
                    self.state.ai.input.replace_range(byte..next, "");
                }
            }
            KeyCode::Left => self.state.ai.cursor = self.state.ai.cursor.saturating_sub(1),
            KeyCode::Right => {
                self.state.ai.cursor =
                    (self.state.ai.cursor + 1).min(self.state.ai.input.chars().count())
            }
            KeyCode::Char(ch)
                if !combo.mods.intersects(
                    ratatui::crossterm::event::KeyModifiers::CONTROL
                        | ratatui::crossterm::event::KeyModifiers::ALT,
                ) =>
            {
                let byte = char_byte(&self.state.ai.input, self.state.ai.cursor);
                self.state.ai.input.insert(byte, ch);
                self.state.ai.cursor += 1;
            }
            _ => {}
        }
    }

    fn cancel_ai_turn(&mut self) {
        if let Some(task) = self.ai_task.take() {
            task.cancel();
        }
        if let Some(waiting) = self.state.ai.waiting.take() {
            if let Some(mcp) = self.mcp.as_mut() {
                mcp.cancel_tool_call(&waiting.server);
            }
            self.state.ai.conversation.push(json!({
                "role": "tool",
                "tool_call_id": waiting.id,
                "content": "Tool call cancelled."
            }));
        }
        for call in self.state.ai.pending.drain(..) {
            self.state.ai.conversation.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": "Tool call cancelled."
            }));
        }
        self.state.ai.busy = false;
        self.state.ai.entries.push("[cancelled]".to_string());
        self.dirty = true;
    }

    fn submit_panel_input(&mut self) {
        if self.state.ai.busy {
            self.notify("AI request is still running");
            return;
        }
        if self.state.ai.queued_question.is_some() {
            self.notify("waiting for MCP tools");
            return;
        }
        let question = self.state.ai.input.trim().to_string();
        if question.is_empty() {
            return;
        }
        self.state.ai.input.clear();
        self.state.ai.cursor = 0;
        self.state.ai.scroll = 0;
        self.state.ai.entries.push(format!("> {question}"));
        self.prime_ai_tools();
        if self.ai_tools_pending() {
            self.state.ai.entries.push("[tools] loading...".to_string());
            self.state.ai.queued_question = Some(question);
            return;
        }
        self.start_ai_question(question);
    }

    fn start_ai_question(&mut self, question: String) {
        self.state.ai.rounds = 0;
        self.state
            .ai
            .conversation
            .push(json!({"role": "user", "content": question}));
        self.start_ai_request();
    }

    pub(crate) fn try_start_queued_ai_turn(&mut self) {
        let Some(question) = self.state.ai.queued_question.clone() else {
            return;
        };
        if self.ai_tools_pending() {
            return;
        }
        self.state.ai.queued_question = None;
        self.state.ai.entries.push("[tools] ready".to_string());
        self.start_ai_question(question);
    }

    pub(crate) fn start_ai_request(&mut self) {
        if self.state.ai.rounds >= MAX_ROUNDS {
            self.state
                .ai
                .entries
                .push(format!("[limit] stopped after {MAX_ROUNDS} model rounds"));
            self.state.ai.busy = false;
            return;
        }
        self.state.ai.rounds += 1;
        let tools = self.ai_tool_definitions();
        let messages = self.state.ai.conversation.clone();
        self.ai_task = Some(crate::ai::spawn_chat(
            self.state.config.ai.clone(),
            messages,
            tools,
        ));
        self.state.ai.busy = true;
        self.state
            .ai
            .entries
            .push("[assistant] thinking...".to_string());
        self.dirty = true;
    }

    fn prime_ai_tools(&mut self) {
        let names: Vec<String> = self
            .mcp
            .as_ref()
            .map(|mcp| mcp.names().into_iter().map(str::to_string).collect())
            .unwrap_or_default();
        let mut failures = Vec::new();
        for name in names {
            if self.mcp_trusted(&name)
                && self
                    .mcp
                    .as_ref()
                    .is_some_and(|mcp| !mcp.failed(&name) && !mcp.tools_loaded(&name))
                && let Some(mcp) = self.mcp.as_mut()
                && let Err(error) = mcp.list_tools(&name)
            {
                failures.push(error);
            }
        }
        for failure in failures {
            self.notify(failure);
        }
    }

    fn ai_tools_pending(&self) -> bool {
        let Some(mcp) = self.mcp.as_ref() else {
            return false;
        };
        mcp.names()
            .into_iter()
            .any(|name| self.mcp_trusted(name) && !mcp.failed(name) && !mcp.tools_loaded(name))
    }

    fn ai_tool_definitions(&mut self) -> Vec<Value> {
        let mut tools = Vec::new();
        let mut map = std::collections::BTreeMap::new();
        if let Some(mcp) = self.mcp.as_ref() {
            for server in mcp.names() {
                if let Some(server_tools) = mcp.tools(server) {
                    add_tool_definitions(server, server_tools, &mut map, &mut tools);
                }
            }
        }
        self.state.ai.tool_map = map;
        tools
    }

    pub(crate) fn pump_ai(&mut self) {
        let Some(task) = self.ai_task.as_ref() else {
            return;
        };
        let result = match task.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("AI request worker stopped".to_string())
            }
        };
        self.ai_task = None;
        self.state.ai.busy = false;
        if self
            .state
            .ai
            .entries
            .last()
            .is_some_and(|line| line == "[assistant] thinking...")
        {
            self.state.ai.entries.pop();
        }
        match result {
            Ok(reply) => {
                self.state.ai.conversation.push(reply.message);
                if let Some(content) = reply.content.filter(|text| !text.is_empty()) {
                    self.state
                        .ai
                        .entries
                        .extend(content.lines().map(str::to_string));
                }
                for call in reply.tool_calls {
                    let Some((server, tool)) = self.state.ai.tool_map.get(&call.name).cloned()
                    else {
                        self.state
                            .ai
                            .entries
                            .push(format!("[error] unknown tool {}", call.name));
                        self.state.ai.conversation.push(json!({
                            "role": "tool",
                            "tool_call_id": call.id,
                            "content": format!("Unknown tool {}", call.name)
                        }));
                        continue;
                    };
                    self.state.ai.entries.push(format!(
                        "[tool] {server}/{} {}",
                        tool,
                        serde_json::to_string(&call.arguments).unwrap_or_default()
                    ));
                    self.state.ai.pending.push_back(AiToolCall {
                        id: call.id,
                        server,
                        tool,
                        arguments: call.arguments,
                    });
                }
                if let Some(call) = self.state.ai.pending.front() {
                    self.state.ai.entries.push(format!(
                        "Allow {}/{}? [y] yes  [n] no",
                        call.server, call.tool
                    ));
                }
            }
            Err(error) => self.state.ai.entries.push(format!("[error] {error}")),
        }
        self.dirty = true;
    }

    fn allow_ai_tool(&mut self) {
        let Some(call) = self.state.ai.pending.pop_front() else {
            return;
        };
        let Some(manager) = self.mcp.as_mut() else {
            self.state
                .ai
                .entries
                .push("[error] MCP manager unavailable".to_string());
            return;
        };
        match manager.call_tool(&call.server, &call.tool, call.arguments.clone()) {
            Ok(()) => {
                self.state.ai.waiting = Some(AiWaiting {
                    id: call.id,
                    server: call.server,
                    tool: call.tool,
                });
                self.state.ai.busy = true;
                self.state.ai.entries.push("[tool] running...".to_string());
            }
            Err(error) => {
                self.state.ai.entries.push(format!("[error] {error}"));
                self.state.ai.conversation.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": format!("Tool call failed: {error}")
                }));
                for rest in self.state.ai.pending.drain(..) {
                    self.state.ai.conversation.push(json!({
                        "role": "tool",
                        "tool_call_id": rest.id,
                        "content": "Tool call was not run."
                    }));
                }
                self.state.ai.busy = false;
            }
        }
    }

    fn deny_ai_tool(&mut self) {
        let calls: Vec<AiToolCall> = self.state.ai.pending.drain(..).collect();
        for call in calls {
            self.state.ai.conversation.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": "Tool call denied by user."
            }));
        }
        self.state
            .ai
            .entries
            .push("[tool] denied by user".to_string());
        self.state.ai.busy = false;
    }

    pub(crate) fn finish_ai_tool(
        &mut self,
        server: &str,
        tool: &str,
        content: &[ToolContent],
        is_error: bool,
    ) {
        let Some(waiting) = self.state.ai.waiting.take() else {
            return;
        };
        if waiting.server != server || waiting.tool != tool {
            self.state.ai.waiting = Some(waiting);
            return;
        }
        let text = tool_content_text(content);
        self.state.ai.conversation.push(json!({
            "role": "tool",
            "tool_call_id": waiting.id,
            "content": text
        }));
        self.state.ai.entries.push(format!(
            "[result] {server}/{tool}{}: {}",
            if is_error { " error" } else { "" },
            text
        ));
        if let Some(call) = self.state.ai.pending.front() {
            self.state.ai.entries.push(format!(
                "Allow {}/{}? [y] yes  [n] no",
                call.server, call.tool
            ));
            self.state.ai.busy = false;
        } else {
            self.start_ai_request();
        }
        self.dirty = true;
    }
}

fn add_tool_definitions(
    server: &str,
    server_tools: &[crate::mcp::Tool],
    map: &mut std::collections::BTreeMap<String, (String, String)>,
    tools: &mut Vec<Value>,
) {
    for tool in server_tools {
        let api = format!("{server}__{}", tool.name);
        map.insert(api.clone(), (server.to_string(), tool.name.clone()));
        tools.push(json!({
            "type": "function",
            "function": {
                "name": api,
                "description": tool.description,
                "parameters": tool.input_schema
            }
        }));
    }
}

fn tool_content_text(content: &[ToolContent]) -> String {
    let mut text = String::new();
    for item in content {
        let piece = match item {
            ToolContent::Text(text) => text.clone(),
            ToolContent::Other(value) => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&piece);
    }
    text
}

fn char_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}
