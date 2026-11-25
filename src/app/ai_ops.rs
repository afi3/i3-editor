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

    pub(crate) fn clear_ai(&mut self) {
        if self.ai_task.is_some()
            || self.state.ai.waiting.is_some()
            || self.state.ai.queued_question.is_some()
            || !self.state.ai.pending.is_empty()
        {
            self.notify("cancel the active AI turn before clearing");
            return;
        }
        self.state.ai.entries.clear();
        self.state
            .ai
            .entries
            .push("AI conversation cleared.".to_string());
        self.state.ai.conversation.clear();
        self.state.ai.input.clear();
        self.state.ai.cursor = 0;
        self.state.ai.scroll = 0;
        self.state.ai.scroll_x = 0;
        self.state.ai.rounds = 0;
        self.state.ai.tool_map.clear();
        self.dirty = true;
    }

    pub(crate) fn copy_ai_transcript(&mut self) {
        let text = self.state.ai.entries.join("\n");
        if text.is_empty() {
            self.notify("AI transcript is empty");
            return;
        }
        self.state.clipboard = text.clone();
        match crate::clipboard::copy_osc52(&text) {
            Some(reason) => self.notify(format!("AI transcript copied internally ({reason})")),
            None => self.notify("AI transcript copied"),
        }
    }

    pub(crate) fn resize_ai(&mut self, delta: i16) {
        let width = self.state.config.ai.panel_width as i16 + delta;
        self.state.config.ai.panel_width = width.clamp(15, 40) as u16;
        self.notify(format!(
            "AI panel width: {}%",
            self.state.config.ai.panel_width
        ));
        self.dirty = true;
    }

    pub(crate) fn scroll_ai_horizontal(&mut self, delta: i16) {
        if delta < 0 {
            self.state.ai.scroll_x = self.state.ai.scroll_x.saturating_sub((-delta) as usize);
        } else {
            self.state.ai.scroll_x = self.state.ai.scroll_x.saturating_add(delta as usize);
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
        if combo
            .mods
            .contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
        {
            match combo.code {
                KeyCode::Left => {
                    self.scroll_ai_horizontal(-4);
                    return;
                }
                KeyCode::Right => {
                    self.scroll_ai_horizontal(4);
                    return;
                }
                _ => {}
            }
        }
        let last = crate::ui::ai::max_scroll(&self.state);
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
        if let Some(task) = self.native_check.take() {
            task.cancel();
        }
        self.native_check_output = None;
        if let Some(waiting) = self.state.ai.waiting.take() {
            if let Some(mcp) = self.mcp.as_mut() {
                mcp.cancel_tool_call(&waiting.server);
            }
            super::audit::result(&self.state, &waiting, "cancelled", None);
            self.state.ai.conversation.push(json!({
                "role": "tool",
                "tool_call_id": waiting.id,
                "content": "Tool call cancelled."
            }));
        }
        let pending: Vec<AiToolCall> = self.state.ai.pending.drain(..).collect();
        for call in pending {
            super::audit::blocked(
                &self.state,
                &call.id,
                &call.server,
                &call.tool,
                &call.arguments,
                "cancelled before execution",
            );
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
        self.state.ai.scroll_x = 0;
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

    fn ai_tool_capability(&self, server: &str, tool: &str) -> String {
        if server == crate::native::SERVER {
            return crate::native::capability(tool).to_string();
        }
        self.mcp
            .as_ref()
            .and_then(|mcp| mcp.tools(server))
            .and_then(|tools| tools.iter().find(|candidate| candidate.name == tool))
            .map(|tool| tool.capability().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn ai_tool_definitions(&mut self) -> Vec<Value> {
        let mut tools = Vec::new();
        let mut map = std::collections::BTreeMap::new();
        if let Some(mcp) = self.mcp.as_ref() {
            for server in mcp.names() {
                if let Some(server_tools) = mcp.tools(server) {
                    add_tool_definitions(
                        server,
                        server_tools,
                        &self.state.config.ai,
                        &mut map,
                        &mut tools,
                    );
                }
            }
        }
        for (name, schema) in crate::native::tools() {
            if !tool_allowed(&self.state.config.ai, crate::native::SERVER, name) {
                continue;
            }
            let api = format!("{}__{name}", crate::native::SERVER);
            map.insert(
                api.clone(),
                (crate::native::SERVER.to_string(), name.to_string()),
            );
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": api,
                    "description": crate::native::description(name),
                    "parameters": schema
                }
            }));
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
                        super::audit::blocked(
                            &self.state,
                            &call.id,
                            "unknown",
                            &call.name,
                            &call.arguments,
                            "tool not offered",
                        );
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
                    let capability = self.ai_tool_capability(&server, &tool);
                    self.state.ai.entries.push(format!(
                        "[tool] {server}/{tool} [{capability}] {}",
                        serde_json::to_string(&call.arguments).unwrap_or_default()
                    ));
                    self.state.ai.pending.push_back(AiToolCall {
                        id: call.id,
                        server,
                        tool,
                        capability,
                        arguments: call.arguments,
                    });
                }
                if let Some(call) = self.state.ai.pending.front() {
                    self.state.ai.entries.push(format!(
                        "Allow {}/{} [{}]? [y] yes  [n] no",
                        call.server, call.tool, call.capability
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
        if !tool_allowed(&self.state.config.ai, &call.server, &call.tool) {
            super::audit::blocked(
                &self.state,
                &call.id,
                &call.server,
                &call.tool,
                &call.arguments,
                "tool policy",
            );
            self.state.ai.entries.push(format!(
                "[blocked] {}/{} by tool policy",
                call.server, call.tool
            ));
            self.state.ai.conversation.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": "Tool call blocked by tool policy."
            }));
            self.state.ai.busy = false;
            return;
        }
        if call.server == crate::native::SERVER {
            self.allow_native_tool(call);
            return;
        }
        let Some(manager) = self.mcp.as_mut() else {
            self.state
                .ai
                .entries
                .push("[error] MCP manager unavailable".to_string());
            return;
        };
        match manager.call_tool(&call.server, &call.tool, call.arguments.clone()) {
            Ok(()) => {
                super::audit::decision(&self.state, &call, "allowed");
                self.state.ai.waiting = Some(AiWaiting {
                    id: call.id,
                    server: call.server,
                    tool: call.tool,
                    capability: call.capability,
                });
                self.state.ai.busy = true;
                self.state.ai.entries.push("[tool] running...".to_string());
            }
            Err(error) => {
                super::audit::blocked(
                    &self.state,
                    &call.id,
                    &call.server,
                    &call.tool,
                    &call.arguments,
                    &error,
                );
                self.state.ai.entries.push(format!("[error] {error}"));
                self.state.ai.conversation.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": format!("Tool call failed: {error}")
                }));
                let pending: Vec<AiToolCall> = self.state.ai.pending.drain(..).collect();
                for rest in pending {
                    super::audit::blocked(
                        &self.state,
                        &rest.id,
                        &rest.server,
                        &rest.tool,
                        &rest.arguments,
                        "not run after previous failure",
                    );
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

    fn allow_native_tool(&mut self, call: AiToolCall) {
        super::audit::decision(&self.state, &call, "allowed");
        let root = self.state.explorer.root.clone();
        let result = self.run_native_tool(&root, &call);
        let waiting = AiWaiting {
            id: call.id.clone(),
            server: call.server.clone(),
            tool: call.tool.clone(),
            capability: call.capability.clone(),
        };
        self.state.ai.waiting = Some(waiting);
        match result {
            Ok(text) => {
                if call.tool == "edit_file" && crate::native::check_eligible(&root, &call.arguments)
                {
                    self.native_check_output = Some(text);
                    self.native_check = Some(crate::native::CheckTask::spawn(
                        root,
                        crate::native::CHECK_TIMEOUT,
                    ));
                    self.state
                        .ai
                        .entries
                        .push("[check] cargo check...".to_string());
                    return;
                }
                self.finish_ai_tool(
                    crate::native::SERVER,
                    &call.tool,
                    &[ToolContent::Text(text)],
                    false,
                );
            }
            Err(error) => {
                self.finish_ai_tool(
                    crate::native::SERVER,
                    &call.tool,
                    &[ToolContent::Text(error)],
                    true,
                );
            }
        }
    }

    pub(crate) fn pump_native_check(&mut self) {
        let Some(task) = self.native_check.as_ref() else {
            return;
        };
        let Some(result) = task.try_recv() else {
            return;
        };
        self.native_check = None;
        let Some(prefix) = self.native_check_output.take() else {
            return;
        };
        let Some(waiting) = self.state.ai.waiting.take() else {
            return;
        };
        let text = format!("{prefix}\n\ncargo check:\n{}", result.output);
        self.state.ai.waiting = Some(waiting);
        self.finish_ai_tool(
            crate::native::SERVER,
            "edit_file",
            &[ToolContent::Text(text)],
            !result.success,
        );
        self.dirty = true;
    }

    fn run_native_tool(
        &mut self,
        root: &std::path::Path,
        call: &AiToolCall,
    ) -> Result<String, String> {
        let raw = call
            .arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "path must be a non-empty string".to_string())?;
        let path = crate::native::resolve_path(root, raw)?;
        let open = self.state.documents.values().find(|document| {
            document
                .path
                .as_deref()
                .is_some_and(|open| super::same_path(open, &path))
        });
        let document_id = open.map(|document| (document.id, document.dirty));
        if let Some((_, true)) = document_id {
            return Err("file has unsaved changes; save or reload it first".to_string());
        }
        if call.tool == "read_file" {
            return crate::native::read_file(root, &call.arguments);
        }
        if call.tool == "search" {
            return crate::native::search(root, &call.arguments);
        }
        if call.tool != "edit_file" {
            return Err(format!("unknown native tool {}", call.tool));
        }
        let result = crate::native::edit_file(root, &call.arguments);
        if result.is_ok()
            && let Some((id, _)) = document_id
        {
            self.reload_document(id)
                .map_err(|error| format!("reload after edit: {error}"))?;
        }
        result
    }

    fn deny_ai_tool(&mut self) {
        let calls: Vec<AiToolCall> = self.state.ai.pending.drain(..).collect();
        for call in calls {
            super::audit::decision(&self.state, &call, "denied");
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
        let text = truncate_tool_text(
            tool_content_text(content),
            self.state.config.ai.max_tool_output_chars,
        );
        super::audit::result(
            &self.state,
            &waiting,
            if is_error { "error" } else { "ok" },
            Some(&format!("{} chars", text.chars().count())),
        );
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
                "Allow {}/{} [{}]? [y] yes  [n] no",
                call.server, call.tool, call.capability
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
    config: &crate::config::AiConfig,
    map: &mut std::collections::BTreeMap<String, (String, String)>,
    tools: &mut Vec<Value>,
) {
    for tool in server_tools {
        if !tool_allowed(config, server, &tool.name) {
            continue;
        }
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

fn tool_allowed(config: &crate::config::AiConfig, server: &str, tool: &str) -> bool {
    let id = format!("{server}/{tool}");
    if config
        .denied_tools
        .iter()
        .any(|pattern| tool_pattern_matches(pattern, &id))
    {
        return false;
    }
    config.allowed_tools.is_empty()
        || config
            .allowed_tools
            .iter()
            .any(|pattern| tool_pattern_matches(pattern, &id))
}

fn tool_pattern_matches(pattern: &str, id: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return id.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return id.ends_with(suffix);
    }
    pattern == id
}

fn truncate_tool_text(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n[truncated]");
    out
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
