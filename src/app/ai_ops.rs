//! AI panel input, bounded MCP tool loop, and transcript.

use ratatui::crossterm::event::KeyCode;
use serde_json::{Value, json};

use crate::command::Command;
use crate::input::{KeyCombo, PromptKind, PromptState};
use crate::mcp::ToolContent;
use crate::state::{AiToolCall, AiWaiting, HelpView, InputMode};

use super::App;

const MAX_ROUNDS: usize = 8;

/// Fixed harness instructions; workspace rules are appended when enabled.
const BASE_SYSTEM_PROMPT: &str = "\
You are a coding assistant inside i3-editor, a terminal editor.
Use the provided tools to inspect the workspace; read before writing and keep
edits scoped to the user's request. Tool calls can be denied: never claim a
tool ran unless its result is present. Writes and destructive tools require
explicit user approval. Stay inside the workspace root.";

fn append_instruction_block(
    prompt: &mut String,
    tag: &str,
    rules: &crate::ai::rules::WorkspaceRules,
) {
    let text = rules.text.trim();
    if text.is_empty() {
        return;
    }
    let source = rules
        .source
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().replace('"', ""))
        .unwrap_or_else(|| "workspace".to_string());
    prompt.push_str(&format!(
        "\n\n<{tag} source=\"{source}\">\n{text}\n</{tag}>"
    ));
}

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
            || self.ai_stream.is_some()
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
        self.ai_compacted = false;
        self.ai_compact_dropped = None;
        self.state.ai.tool_map.clear();
        self.state.ai.attachments.clear();
        self.remove_ai_history();
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
        if combo.code != KeyCode::Esc
            || (self.ai_task.is_none()
                && self.ai_stream.is_none()
                && self.state.ai.waiting.is_none())
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
            KeyCode::Enter
                if combo.mods.intersects(
                    ratatui::crossterm::event::KeyModifiers::SHIFT
                        | ratatui::crossterm::event::KeyModifiers::ALT,
                ) =>
            {
                insert_ai_char(self, '\n');
            }
            KeyCode::Char('j')
                if combo
                    .mods
                    .contains(ratatui::crossterm::event::KeyModifiers::CONTROL) =>
            {
                insert_ai_char(self, '\n');
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
        self.record_ai_trace("cancelled", None, None, 0);
        self.ai_compact_dropped = None;
        self.ai_compacted = false;
        self.remove_compacting_entry();
        if let Some(task) = self.ai_task.take() {
            task.cancel();
        }
        if let Some(task) = self.ai_stream.take() {
            task.cancel();
        }
        self.ai_streamed = false;
        if let Some(task) = self.native_check.take() {
            task.cancel();
        }
        if let Some(task) = self.native_verify.take() {
            task.cancel();
        }
        self.native_check_output = None;
        self.native_diagnostics = None;
        if let Some(waiting) = self.state.ai.waiting.take() {
            if let Some(mcp) = self.mcp.as_mut() {
                mcp.cancel_tool_call(&waiting.server);
            }
            self.mcp_subscriptions
                .retain(|(server, _)| server != &waiting.server);
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
        if self.background_agent {
            self.objective_queue = false;
            self.objectives.queue_active = false;
            let _ = crate::objectives::save(&self.objectives);
            self.finish_objective("blocked");
        }
        self.background_agent = false;
        self.state.ai.entries.push("[cancelled]".to_string());
        self.save_ai_history();
        self.dirty = true;
    }

    pub(crate) fn open_ai_background(&mut self) {
        if !self.state.config.ai.background_agents {
            self.notify("enable [ai] background_agents first");
            return;
        }
        if self.background_agent
            || self.state.ai.busy
            || self.ai_task.is_some()
            || self.ai_stream.is_some()
        {
            self.notify("an AI turn is already running");
            return;
        }
        self.state.mode = InputMode::Prompt(PromptState::new(PromptKind::AiBackground));
        self.notify("background agent: read-only tools only");
        self.dirty = true;
    }

    pub(crate) fn submit_ai_background(&mut self, prompt: PromptState) {
        let task = prompt.fields.first().cloned().unwrap_or_default();
        self.start_background_task(task);
    }

    fn start_background_task(&mut self, task: String) {
        if task.trim().is_empty() {
            self.notify("background task is empty");
            return;
        }
        self.state.mode = InputMode::Normal;
        self.background_agent = true;
        self.state.ai.visible = false;
        self.state.ai.focused = false;
        self.state
            .ai
            .entries
            .push("[background] started; only read-only tools run automatically".to_string());
        self.start_ai_question(task);
    }

    pub(crate) fn index_ai_context(&mut self) {
        let index = crate::context_index::build(&self.state.explorer.root);
        let count = index.files.len();
        if let Err(error) = crate::context_index::save(&index) {
            self.notify(format!("context index not saved: {error}"));
            return;
        }
        self.notify(format!("context index: {count} files"));
        self.dirty = true;
    }

    pub(crate) fn open_ai_context_files(&mut self) {
        let Some(index) = crate::context_index::load() else {
            self.notify("build the context index first with ai_index_project");
            return;
        };
        let mut prompt = PromptState::new(PromptKind::AiContextFiles);
        prompt.set_candidates(index.files.iter().map(|entry| entry.path.clone()).collect());
        self.state.mode = InputMode::Prompt(prompt);
        self.dirty = true;
    }

    pub(crate) fn ai_request_messages(&self) -> Vec<Value> {
        let mut messages = Vec::with_capacity(self.state.ai.conversation.len() + 1);
        messages.push(json!({"role": "system", "content": self.ai_system_prompt()}));
        messages.extend(self.state.ai.conversation.iter().cloned());
        messages
    }

    pub(crate) fn ai_system_prompt(&self) -> String {
        let mut prompt = BASE_SYSTEM_PROMPT.to_string();
        if self.state.config.ai.workspace_rules {
            let rules = crate::ai::rules::load(&self.state.explorer.root);
            append_instruction_block(&mut prompt, "workspace_rules", &rules);
        }
        if self.state.config.ai.workspace_memory {
            let memory = crate::ai::rules::load_memory(&self.state.explorer.root);
            append_instruction_block(&mut prompt, "workspace_memory", &memory);
        }
        prompt
    }

    pub(crate) fn open_ai_rules(&mut self) {
        let rules = crate::ai::rules::load(&self.state.explorer.root);
        let memory = crate::ai::rules::load_memory(&self.state.explorer.root);
        let mut lines = Vec::new();
        match &rules.source {
            Some(path) => {
                lines.push(format!("rules: {}", path.display()));
                lines.push(format!(
                    "rules size: {} bytes{}",
                    rules.text.len(),
                    if rules.truncated {
                        " (truncated at 16 KiB)"
                    } else {
                        ""
                    }
                ));
            }
            None => lines.push("no AGENTS.md or CLAUDE.md in the workspace root".to_string()),
        }
        if let Some(warning) = &rules.warning {
            lines.push(format!("rules warning: {warning}"));
        }
        lines.push(format!(
            "rules prompt: {}",
            if self.state.config.ai.workspace_rules {
                "enabled"
            } else {
                "disabled (workspace_rules = false)"
            }
        ));
        if !rules.text.is_empty() {
            lines.push(String::new());
            lines.extend(rules.text.lines().map(str::to_string));
        }

        lines.push(String::new());
        match &memory.source {
            Some(path) => {
                lines.push(format!("memory: {}", path.display()));
                lines.push(format!(
                    "memory size: {} bytes{}",
                    memory.text.len(),
                    if memory.truncated {
                        " (truncated at 16 KiB)"
                    } else {
                        ""
                    }
                ));
            }
            None => lines.push("no MEMORY.md in the workspace root".to_string()),
        }
        if let Some(warning) = &memory.warning {
            lines.push(format!("memory warning: {warning}"));
        }
        lines.push(format!(
            "memory prompt: {}",
            if self.state.config.ai.workspace_memory {
                "enabled"
            } else {
                "disabled (workspace_memory = false)"
            }
        ));
        if !memory.text.is_empty() {
            lines.push(String::new());
            lines.extend(memory.text.lines().map(str::to_string));
        }
        self.state.help = None;
        self.state.trace_view = None;
        self.state.completions = None;
        self.state.hover = None;
        self.state.rules_view = Some(HelpView { lines, scroll: 0 });
        self.dirty = true;
    }

    pub(crate) fn open_ai_trace(&mut self) {
        let lines = super::audit::recent_model_traces(&self.state, 20);
        let lines = if lines.is_empty() {
            vec!["no model requests recorded".to_string()]
        } else {
            lines
        };
        self.state.help = None;
        self.state.rules_view = None;
        self.state.completions = None;
        self.state.hover = None;
        self.state.trace_view = Some(HelpView { lines, scroll: 0 });
        self.dirty = true;
    }

    pub(crate) fn submit_ai_context_file(&mut self, prompt: PromptState) {
        let Some(path) = prompt.selected_candidate() else {
            return;
        };
        let text = match crate::native::read_source(&self.state.explorer.root, path) {
            Ok(text) => text,
            Err(error) => {
                self.notify(error);
                return;
            }
        };
        let text = crate::ai::truncate_text(&text, self.state.config.ai.max_tool_output_chars);
        self.state.mode = InputMode::Normal;
        self.attach_ai_context(format!("{path} (indexed file)"), text);
    }

    pub(crate) fn open_ai_objective_add(&mut self) {
        self.state.mode = InputMode::Prompt(PromptState::new(PromptKind::AiObjectiveAdd));
        self.dirty = true;
    }

    pub(crate) fn submit_ai_objective_add(&mut self, prompt: PromptState) {
        let text = prompt.fields.first().cloned().unwrap_or_default();
        if text.trim().is_empty() {
            self.notify("objective is empty");
            return;
        }
        self.objectives.add(text.trim().to_string());
        let _ = crate::objectives::save(&self.objectives);
        self.state.mode = InputMode::Normal;
        self.notify("agent objective saved");
        self.dirty = true;
    }

    pub(crate) fn resume_objective_queue(&mut self) {
        if !self.state.config.ai.background_agents
            || !self.state.config.ai.agent_auto_queue
            || !self.objectives.queue_active
        {
            return;
        }
        let changed = self.objectives.reset_running();
        if changed {
            let _ = crate::objectives::save(&self.objectives);
        }
        if self
            .objectives
            .objectives
            .iter()
            .any(|objective| objective.status == "pending")
        {
            self.notify("resuming objective queue");
            self.start_objective_queue();
        } else {
            self.objectives.queue_active = false;
            let _ = crate::objectives::save(&self.objectives);
        }
    }

    pub(crate) fn start_objective_queue(&mut self) {
        if !self.state.config.ai.background_agents || !self.state.config.ai.agent_auto_queue {
            self.notify("enable background_agents and agent_auto_queue first");
            return;
        }
        if self.background_agent || self.state.ai.busy {
            self.notify("an AI turn is already running");
            return;
        }
        self.objective_queue = true;
        self.objectives.queue_active = true;
        let _ = crate::objectives::save(&self.objectives);
        self.run_next_objective();
    }

    fn run_next_objective(&mut self) {
        let next = self
            .objectives
            .objectives
            .iter()
            .find(|objective| {
                objective.status == "pending"
                    && objective.depends_on.iter().all(|dep| {
                        self.objectives
                            .get(*dep)
                            .is_some_and(|dependency| dependency.status == "done")
                    })
            })
            .cloned();
        let Some(objective) = next else {
            self.objective_queue = false;
            self.objectives.queue_active = false;
            let _ = crate::objectives::save(&self.objectives);
            let waiting = self
                .objectives
                .objectives
                .iter()
                .any(|objective| objective.status == "pending");
            self.notify(if waiting {
                "objective queue is waiting for dependencies"
            } else {
                "objective queue finished"
            });
            return;
        };
        self.active_objective = Some(objective.id);
        self.objectives.set_status(objective.id, "running");
        let _ = crate::objectives::save(&self.objectives);
        self.start_background_task(objective.text);
    }

    pub(crate) fn open_ai_objective_link(&mut self) {
        self.state.mode = InputMode::Prompt(PromptState::new(PromptKind::AiObjectiveLink));
        self.notify("enter: objective-id dependency-id,dependency-id");
        self.dirty = true;
    }

    pub(crate) fn submit_ai_objective_link(&mut self, prompt: PromptState) {
        let raw = prompt.fields.first().cloned().unwrap_or_default();
        let Some((id, deps)) = raw.split_once(':') else {
            self.notify("use objective-id: dependency-id,dependency-id");
            return;
        };
        let Ok(id) = id.trim().parse::<u64>() else {
            self.notify("objective id must be a number");
            return;
        };
        let deps: Result<Vec<u64>, _> = deps
            .split(',')
            .filter(|part| !part.trim().is_empty())
            .map(|part| part.trim().parse::<u64>())
            .collect();
        let Ok(deps) = deps else {
            self.notify("dependency ids must be numbers");
            return;
        };
        if self.objectives.has_cycle(id, &deps) {
            self.notify("objective dependency cycle");
            return;
        }
        if let Some(objective) = self
            .objectives
            .objectives
            .iter_mut()
            .find(|objective| objective.id == id)
        {
            objective.depends_on = deps;
            let _ = crate::objectives::save(&self.objectives);
            self.state.mode = InputMode::Normal;
            self.notify("objective dependencies saved");
        } else {
            self.notify("objective not found");
        }
    }

    pub(crate) fn open_ai_objectives(&mut self) {
        let candidates = self
            .objectives
            .objectives
            .iter()
            .map(|objective| {
                let deps = if objective.depends_on.is_empty() {
                    String::new()
                } else {
                    format!(" deps:{:?}", objective.depends_on)
                };
                format!(
                    "{} [{}] {}{}",
                    objective.id, objective.status, objective.text, deps
                )
            })
            .collect();
        let mut prompt = PromptState::new(PromptKind::AiObjectives);
        prompt.candidates = candidates;
        self.state.mode = InputMode::Prompt(prompt);
        self.dirty = true;
    }

    pub(crate) fn submit_ai_objective(&mut self, prompt: PromptState) {
        let Some(candidate) = prompt.selected_candidate() else {
            return;
        };
        let Some(id) = candidate
            .split_whitespace()
            .next()
            .and_then(|id| id.parse::<u64>().ok())
        else {
            return;
        };
        let Some(objective) = self.objectives.get(id).cloned() else {
            return;
        };
        if !self.state.config.ai.background_agents {
            self.notify("enable [ai] background_agents first");
            return;
        }
        if self.background_agent || self.state.ai.busy || self.objective_queue {
            self.notify("an AI turn is already running");
            return;
        }
        self.state.mode = InputMode::Normal;
        self.active_objective = Some(id);
        self.objectives.set_status(id, "running");
        let _ = crate::objectives::save(&self.objectives);
        self.start_background_task(objective.text);
    }

    fn finish_objective(&mut self, status: &str) {
        if let Some(id) = self.active_objective.take() {
            self.objectives.set_status(id, status);
            let _ = crate::objectives::save(&self.objectives);
        }
    }

    fn auto_allow_background_tool(&mut self) {
        let Some(call) = self.state.ai.pending.front().cloned() else {
            self.finish_background();
            return;
        };
        if !tool_allowed(&self.state.config.ai, &call.server, &call.tool) {
            self.stop_background_with_error("background agent tool policy blocked the request");
            return;
        }
        if call.capability != "read-only"
            && !self
                .state
                .config
                .ai
                .background_auto_tools
                .iter()
                .any(|pattern| {
                    tool_pattern_matches(pattern, &format!("{}/{}", call.server, call.tool))
                })
        {
            self.state.ai.busy = false;
            self.state.ai.entries.push(format!(
                "Approve background {}/{} [{}]? [y] yes [n] no",
                call.server, call.tool, call.capability
            ));
            self.notify("background agent waiting for tool approval");
            self.dirty = true;
            return;
        }
        self.allow_ai_tool();
    }

    fn stop_background_with_error(&mut self, message: &str) {
        let pending_calls: Vec<AiToolCall> = self.state.ai.pending.drain(..).collect();
        for pending in pending_calls {
            super::audit::blocked(
                &self.state,
                &pending.id,
                &pending.server,
                &pending.tool,
                &pending.arguments,
                "background agent policy",
            );
            self.state.ai.conversation.push(json!({
                "role": "tool",
                "tool_call_id": pending.id,
                "content": message
            }));
        }
        self.background_agent = false;
        self.objective_queue = false;
        self.objectives.queue_active = false;
        let _ = crate::objectives::save(&self.objectives);
        self.finish_objective("blocked");
        self.state.ai.busy = false;
        self.state
            .ai
            .entries
            .push(format!("[background] stopped: {message}"));
        self.notify(message);
        self.save_ai_history();
    }

    fn finish_background(&mut self) {
        if !self.background_agent {
            return;
        }
        self.background_agent = false;
        self.finish_objective("done");
        self.state
            .ai
            .entries
            .push("[background] finished".to_string());
        self.notify("background agent finished");
        self.save_ai_history();
        if self.objective_queue {
            self.run_next_objective();
        } else {
            self.objectives.queue_active = false;
            let _ = crate::objectives::save(&self.objectives);
        }
    }

    pub(crate) fn attach_ai_file(&mut self) {
        let Some(document) = self.state.active_document() else {
            self.notify("no active document to attach");
            return;
        };
        let path = document
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<untitled>".to_string());
        let text = crate::ai::truncate_text(
            &document.rope().to_string(),
            self.state.config.ai.max_tool_output_chars,
        );
        let label = format!("{path} (file)");
        self.attach_ai_context(label, text);
    }

    pub(crate) fn attach_ai_selection(&mut self) {
        let Some(pane) = self.state.focused_pane() else {
            self.notify("focus an editor pane before attaching a selection");
            return;
        };
        let Some(document) = self.state.active_document() else {
            self.notify("no active document to attach");
            return;
        };
        let ranges = pane.selected_ranges();
        if ranges.is_empty() {
            self.notify("select text before attaching a selection");
            return;
        }
        let path = document
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<untitled>".to_string());
        let text = ranges
            .iter()
            .map(|range| document.rope().slice(range.clone()).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let text = crate::ai::truncate_text(&text, self.state.config.ai.max_tool_output_chars);
        let start = document.line_of_char(ranges[0].start) + 1;
        let end = document.line_of_char(ranges[ranges.len() - 1].end.saturating_sub(1)) + 1;
        let label = format!("{path} (selection lines {start}-{end})");
        self.attach_ai_context(label, text);
    }

    fn attach_ai_context(&mut self, label: String, text: String) {
        self.state.ai.attachments.push(crate::state::AiAttachment {
            path: label.clone(),
            text,
        });
        self.state
            .ai
            .entries
            .push(format!("[context] attached {label}"));
        self.notify(format!("attached {label}"));
        self.dirty = true;
    }

    pub(crate) fn clear_ai_context(&mut self) {
        if self.state.ai.attachments.is_empty() {
            self.notify("no AI context attached");
            return;
        }
        self.state.ai.attachments.clear();
        self.state.ai.entries.push("[context] cleared".to_string());
        self.dirty = true;
    }

    fn auto_ai_context(&mut self, query: &str) -> Vec<crate::state::AiAttachment> {
        let mut attached = Vec::new();
        let mut used = 0usize;
        let selected = crate::context_index::load().map(|index| {
            crate::context_index::select(&index, query, self.state.config.ai.auto_context_files)
        });
        for entry in selected.into_iter().flatten() {
            let remaining = self.state.config.ai.auto_context_chars.saturating_sub(used);
            if remaining == 0 {
                break;
            }
            let Ok(text) = crate::native::read_source(&self.state.explorer.root, &entry.path)
            else {
                continue;
            };
            let text = crate::ai::truncate_text(
                &text,
                remaining.min(self.state.config.ai.max_tool_output_chars),
            );
            let chars = text.chars().count();
            if chars == 0 {
                continue;
            }
            used += chars;
            self.state
                .ai
                .entries
                .push(format!("[context] auto attached {}", entry.path));
            attached.push(crate::state::AiAttachment {
                path: format!("{} (auto)", entry.path),
                text,
            });
        }
        attached
    }

    fn submit_panel_input(&mut self) {
        if self.background_agent {
            self.notify("background agent is active; approve or deny its pending tool");
            return;
        }
        if self.state.ai.busy {
            self.notify("AI request is still running");
            return;
        }
        if self.state.ai.queued_question.is_some() {
            self.notify("waiting for MCP tools");
            return;
        }
        if self.semantic_task.is_some() {
            self.notify("still preparing semantic context; wait for it to finish");
            return;
        }
        let question = self.state.ai.input.trim().to_string();
        let attachments = std::mem::take(&mut self.state.ai.attachments);
        if question.is_empty() && attachments.is_empty() {
            return;
        }
        if attachments.is_empty()
            && self.state.config.ai.semantic_memory
            && !self.state.config.ai.embedding_model.trim().is_empty()
            && let Some(index_path) = crate::semantic::default_path()
        {
            let lexical = if self.state.config.ai.auto_context {
                self.auto_ai_context(&question)
            } else {
                Vec::new()
            };
            let used = lexical
                .iter()
                .map(|attachment| attachment.text.chars().count())
                .sum();
            let model = self.state.config.ai.embedding_model.trim().to_string();
            self.state
                .ai
                .entries
                .push(format!("[context] embedding the question for {model}"));
            self.semantic_task = Some(crate::app::semantic_ops::QueryTask::spawn(
                self.state.config.ai.clone(),
                model,
                index_path,
                question.clone(),
                lexical,
                used,
            ));
            self.consume_ai_input(&question);
            self.dirty = true;
            return;
        }
        let auto = if attachments.is_empty() && self.state.config.ai.auto_context {
            self.auto_ai_context(&question)
        } else {
            Vec::new()
        };
        let mut all = attachments;
        all.extend(auto);
        self.consume_ai_input(&question);
        self.finish_ai_question(question, all);
    }

    /// Clear the input row after the question was taken.
    fn consume_ai_input(&mut self, question: &str) {
        self.state.ai.input.clear();
        self.state.ai.cursor = 0;
        self.state.ai.scroll = 0;
        self.state.ai.scroll_x = 0;
        if !question.is_empty() {
            self.state.ai.entries.push(format!("> {question}"));
        }
    }

    /// Turn a finished question plus its context into a request, or queue it
    /// while the tool list is still loading.
    pub(crate) fn finish_ai_question(
        &mut self,
        question: String,
        attachments: Vec<crate::state::AiAttachment>,
    ) {
        let context = attachments
            .iter()
            .map(|attachment| {
                format!(
                    "<context path=\"{}\">\n{}\n</context>",
                    attachment.path, attachment.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let content = if context.is_empty() {
            question.clone()
        } else if question.is_empty() {
            context
        } else {
            format!("{context}\n\n{question}")
        };
        self.prime_ai_tools();
        if self.ai_tools_pending() {
            self.state.ai.entries.push("[tools] loading...".to_string());
            self.state.ai.queued_question = Some(content);
            return;
        }
        self.start_ai_question(content);
    }

    fn start_ai_question(&mut self, question: String) {
        self.state.ai.rounds = 0;
        self.ai_compacted = false;
        self.ai_compact_dropped = None;
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

    fn ai_budget_reason(&self) -> Option<String> {
        let tokens = self
            .state
            .ai
            .prompt_tokens
            .saturating_add(self.state.ai.completion_tokens);
        let max_tokens = self.state.config.ai.max_session_tokens;
        if max_tokens > 0 && tokens >= max_tokens as u64 {
            return Some(format!(
                "session token budget reached ({tokens}/{max_tokens})"
            ));
        }
        let input_price = self.state.config.ai.input_cost_per_million;
        let output_price = self.state.config.ai.output_cost_per_million;
        let cost = (self.state.ai.prompt_tokens as f64 * input_price
            + self.state.ai.completion_tokens as f64 * output_price)
            / 1_000_000.0;
        let max_cost = self.state.config.ai.max_session_cost_usd;
        if max_cost > 0.0 && cost >= max_cost {
            return Some(format!(
                "session cost budget reached (${cost:.4}/${max_cost:.4})"
            ));
        }
        None
    }

    fn stop_for_budget(&mut self, reason: &str) {
        self.state.ai.entries.push(format!("[limit] {reason}"));
        self.notify(reason);
        if self.background_agent {
            self.stop_background_with_error(reason);
        } else {
            self.state.ai.busy = false;
        }
        self.dirty = true;
    }

    pub(crate) fn start_ai_request(&mut self) {
        // A live task must never be replaced: dropping it cancels the request.
        if self.ai_task.is_some() {
            return;
        }
        if self.ai_compact_dropped.is_some() {
            return;
        }
        if let Some(reason) = self.ai_budget_reason() {
            self.stop_for_budget(&reason);
            return;
        }
        if self.state.ai.rounds >= MAX_ROUNDS {
            self.state
                .ai
                .entries
                .push(format!("[limit] stopped after {MAX_ROUNDS} model rounds"));
            self.state.ai.busy = false;
            return;
        }
        let config = self.state.config.ai.clone();
        let tools = self.ai_tool_definitions();
        let messages = self.ai_request_messages();
        let plan = crate::ai::trim_plan(&messages, &tools, &config);
        if config.compact_history && !self.ai_compacted && !plan.dropped.is_empty() {
            self.ai_compacted = true;
            self.ai_compact_dropped = Some(plan.dropped.len());
            let summary = crate::ai::compaction_messages(
                &plan.dropped,
                config.max_tool_output_chars.max(2_000),
            );
            let stats = crate::ai::trim_stats(&summary, &[], &config);
            self.ai_request_trace = Some(super::AiRequestTrace {
                round: self.state.ai.rounds,
                model: config.model.clone(),
                started: std::time::Instant::now(),
                original_messages: stats.original,
                sent_messages: stats.sent,
                estimated_tokens: stats.estimated_tokens,
            });
            self.ai_task = Some(crate::ai::spawn_chat(config, summary, Vec::new()));
            self.ai_stream = None;
            self.state.ai.busy = true;
            self.state
                .ai
                .entries
                .push("[assistant] compacting...".to_string());
            self.dirty = true;
            return;
        }
        self.ai_compacted = false;
        self.state.ai.rounds += 1;
        let stats = crate::ai::trim_stats(&messages, &tools, &config);
        self.ai_request_trace = Some(super::AiRequestTrace {
            round: self.state.ai.rounds,
            model: config.model.clone(),
            started: std::time::Instant::now(),
            original_messages: stats.original,
            sent_messages: stats.sent,
            estimated_tokens: stats.estimated_tokens,
        });
        self.ai_task = None;
        self.ai_stream = None;
        self.ai_streamed = false;
        if config.stream {
            self.ai_stream = Some(crate::ai::spawn_stream(config, messages, tools));
        } else {
            self.ai_task = Some(crate::ai::spawn_chat(config, messages, tools));
        }
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

    pub(crate) fn ai_tool_definitions(&mut self) -> Vec<Value> {
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
        let verify_names: Vec<String> = self.state.config.ai.verify.keys().cloned().collect();
        if !verify_names.is_empty()
            && tool_allowed(&self.state.config.ai, crate::native::SERVER, "verify")
        {
            let api = format!("{}__verify", crate::native::SERVER);
            map.insert(
                api.clone(),
                (crate::native::SERVER.to_string(), "verify".to_string()),
            );
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": api,
                    "description": crate::native::description("verify"),
                    "parameters": crate::native::verify_schema(&verify_names)
                }
            }));
        }
        self.state.ai.tool_map = map;
        tools
    }

    pub(crate) fn pump_ai(&mut self) {
        if self.ai_stream.is_some() {
            self.pump_ai_stream();
            return;
        }
        let Some(task) = self.ai_task.as_ref() else {
            return;
        };
        let Some(result) = crate::ai::poll_task(task) else {
            return;
        };
        self.ai_task = None;
        if self.ai_compact_dropped.is_some() {
            self.finish_ai_compaction(result);
            self.dirty = true;
            return;
        }
        self.state.ai.busy = false;
        self.remove_thinking_entry();
        match result {
            Ok(reply) => self.finish_ai_reply(reply, false),
            Err(error) => {
                self.record_ai_trace("error", None, None, 0);
                if self.background_agent {
                    self.objective_queue = false;
                    self.finish_objective("blocked");
                }
                self.background_agent = false;
                self.state.ai.entries.push(format!("[error] {error}"));
            }
        }
        self.dirty = true;
    }

    fn pump_ai_stream(&mut self) {
        let Some(task) = self.ai_stream.as_ref() else {
            return;
        };
        let mut events = Vec::new();
        while let Some(event) = task.try_recv() {
            events.push(event);
        }
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                crate::ai::StreamEvent::Delta(delta) => self.push_stream_delta(&delta),
                crate::ai::StreamEvent::Done(reply) => {
                    self.ai_stream = None;
                    self.state.ai.busy = false;
                    self.remove_thinking_entry();
                    let streamed = self.ai_streamed;
                    self.ai_streamed = false;
                    self.finish_ai_reply(reply, streamed);
                    break;
                }
                crate::ai::StreamEvent::Error(error) => {
                    let was_streamed = self.ai_streamed;
                    self.ai_stream = None;
                    self.ai_streamed = false;
                    self.record_ai_trace("error", None, None, 0);
                    if self.background_agent {
                        self.objective_queue = false;
                        self.finish_objective("blocked");
                    }
                    self.background_agent = false;
                    self.state.ai.busy = false;
                    self.remove_thinking_entry();
                    if was_streamed {
                        self.state
                            .ai
                            .entries
                            .push("[stream interrupted]".to_string());
                    }
                    self.state.ai.entries.push(format!("[error] {error}"));
                    break;
                }
            }
        }
        self.dirty = true;
    }

    fn push_stream_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.remove_thinking_entry();
        if self.ai_streamed
            && let Some(last) = self.state.ai.entries.last_mut()
            && last.starts_with("[assistant] ")
        {
            last.push_str(delta);
            return;
        }
        self.ai_streamed = true;
        self.state.ai.entries.push(format!("[assistant] {delta}"));
    }

    fn remove_thinking_entry(&mut self) {
        if self
            .state
            .ai
            .entries
            .last()
            .is_some_and(|line| line == "[assistant] thinking...")
        {
            self.state.ai.entries.pop();
        }
    }

    fn record_ai_trace(
        &mut self,
        stop: &str,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        tool_calls: usize,
    ) {
        let Some(trace) = self.ai_request_trace.take() else {
            return;
        };
        let call = super::audit::ModelRequest {
            round: trace.round,
            model: &trace.model,
            original_messages: trace.original_messages,
            sent_messages: trace.sent_messages,
            estimated_tokens: trace.estimated_tokens,
            latency: trace.started.elapsed(),
            stop,
            prompt_tokens,
            completion_tokens,
            tool_calls,
        };
        super::audit::model_request(&self.state, &call);
    }

    fn record_reply_trace(&mut self, reply: &crate::ai::Reply) {
        let stop = if self.ai_budget_reason().is_some() {
            "budget"
        } else {
            "reply"
        };
        self.record_ai_trace(
            stop,
            reply.prompt_tokens,
            reply.completion_tokens,
            reply.tool_calls.len(),
        );
    }

    fn remove_compacting_entry(&mut self) {
        if self
            .state
            .ai
            .entries
            .last()
            .is_some_and(|line| line == "[assistant] compacting...")
        {
            self.state.ai.entries.pop();
        }
    }

    fn finish_ai_compaction(&mut self, result: Result<crate::ai::Reply, String>) {
        self.remove_compacting_entry();
        let dropped = self.ai_compact_dropped.take().unwrap_or(0);
        match result {
            Ok(reply) => {
                self.state.ai.prompt_tokens = self
                    .state
                    .ai
                    .prompt_tokens
                    .saturating_add(reply.prompt_tokens.unwrap_or(0));
                self.state.ai.completion_tokens = self
                    .state
                    .ai
                    .completion_tokens
                    .saturating_add(reply.completion_tokens.unwrap_or(0));
                self.record_ai_trace("compact", reply.prompt_tokens, reply.completion_tokens, 0);
                self.replace_compacted(dropped, reply.content.unwrap_or_default().trim());
                if let Some(reason) = self.ai_budget_reason() {
                    self.stop_for_budget(&reason);
                    self.dirty = true;
                    return;
                }
            }
            Err(error) => {
                self.record_ai_trace("error", None, None, 0);
                self.state
                    .ai
                    .entries
                    .push(format!("[compact] {error}; continuing without summary"));
            }
        }
        self.state.ai.busy = false;
        self.start_ai_request();
        self.dirty = true;
    }

    fn replace_compacted(&mut self, dropped: usize, summary: &str) {
        let mut removed = 0;
        let mut index = 0;
        while removed < dropped && index < self.state.ai.conversation.len() {
            if self.state.ai.conversation[index]
                .get("role")
                .and_then(Value::as_str)
                == Some("system")
            {
                index += 1;
                continue;
            }
            self.state.ai.conversation.remove(index);
            removed += 1;
        }
        if summary.is_empty() {
            return;
        }
        let summary = crate::ai::truncate_text(summary, 4_000);
        let position = self
            .state
            .ai
            .conversation
            .iter()
            .take_while(|message| message.get("role").and_then(Value::as_str) == Some("system"))
            .count();
        self.state.ai.conversation.insert(
            position,
            json!({
                "role": "system",
                "content": format!("Summary of earlier conversation:\n{summary}")
            }),
        );
    }

    fn finish_ai_reply(&mut self, reply: crate::ai::Reply, streamed: bool) {
        self.state.ai.prompt_tokens = self
            .state
            .ai
            .prompt_tokens
            .saturating_add(reply.prompt_tokens.unwrap_or(0));
        self.state.ai.completion_tokens = self
            .state
            .ai
            .completion_tokens
            .saturating_add(reply.completion_tokens.unwrap_or(0));
        self.record_reply_trace(&reply);
        self.state.ai.conversation.push(reply.message);
        if !streamed && let Some(content) = reply.content.filter(|text| !text.is_empty()) {
            self.state
                .ai
                .entries
                .extend(content.lines().map(str::to_string));
        }
        if let Some(reason) = self.ai_budget_reason() {
            for call in &reply.tool_calls {
                self.state.ai.conversation.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": format!("Tool not run: {reason}")
                }));
                self.state
                    .ai
                    .entries
                    .push(format!("[blocked] {}/{}: {reason}", call.name, call.id));
            }
            self.stop_for_budget(&reason);
            return;
        }
        for call in reply.tool_calls {
            let Some((server, tool)) = self.state.ai.tool_map.get(&call.name).cloned() else {
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
            if server == crate::native::SERVER && (tool == "edit_file" || tool == "write_file") {
                let preview = if tool == "edit_file" {
                    crate::native::preview_edit(&self.state.explorer.root, &call.arguments)
                } else {
                    crate::native::preview_write(&self.state.explorer.root, &call.arguments)
                };
                match preview {
                    Ok(diff) => {
                        let diff = crate::ai::truncate_text(
                            &diff,
                            self.state.config.ai.max_tool_output_chars,
                        );
                        self.state
                            .ai
                            .entries
                            .extend(diff.lines().map(str::to_string));
                    }
                    Err(error) => self.state.ai.entries.push(format!("[diff] {error}")),
                }
            }
            if server == crate::native::SERVER && tool == "verify" {
                let name = call.arguments.get("name").and_then(Value::as_str);
                match name.and_then(|name| self.state.config.ai.verify.get(name)) {
                    Some(spec) => self.state.ai.entries.push(format!(
                        "[verify] {} {}",
                        spec.command,
                        spec.args.join(" ")
                    )),
                    None => self
                        .state
                        .ai
                        .entries
                        .push("[verify] unknown command name".to_string()),
                }
            }
            self.state.ai.pending.push_back(AiToolCall {
                id: call.id,
                server,
                tool,
                capability,
                arguments: call.arguments,
            });
        }
        if self.state.ai.pending.is_empty() {
            self.finish_background();
        } else if self.background_agent {
            self.auto_allow_background_tool();
        } else if let Some(call) = self.state.ai.pending.front() {
            self.state.ai.entries.push(format!(
                "Allow {}/{} [{}]? [y] yes  [n] no",
                call.server, call.tool, call.capability
            ));
        }
        self.save_ai_history();
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
        if call.tool == "verify" {
            self.state.ai.waiting = Some(AiWaiting {
                id: call.id.clone(),
                server: call.server.clone(),
                tool: call.tool.clone(),
                capability: call.capability.clone(),
            });
            let name = call
                .arguments
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string);
            let spec = name
                .as_ref()
                .and_then(|name| self.state.config.ai.verify.get(name))
                .cloned();
            match (name, spec) {
                (Some(name), Some(spec)) => {
                    let max_chars = self.state.config.ai.max_tool_output_chars;
                    self.native_verify = Some(crate::native::VerifyTask::spawn(
                        root, name, spec, max_chars,
                    ));
                    self.state.ai.busy = true;
                    self.state
                        .ai
                        .entries
                        .push("[verify] running configured command...".to_string());
                }
                _ => {
                    self.finish_ai_tool(
                        crate::native::SERVER,
                        "verify",
                        &[ToolContent::Text(
                            "unknown verification command name".to_string(),
                        )],
                        true,
                    );
                }
            }
            self.dirty = true;
            return;
        }
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
                if (call.tool == "edit_file" || call.tool == "write_file")
                    && self.state.config.ai.verify_after_edit
                    && self.native_verify.is_none()
                {
                    let name = self.state.config.ai.verify_after_edit_command.clone();
                    if let Some(spec) = self.state.config.ai.verify.get(&name).cloned() {
                        let max_chars = self.state.config.ai.max_tool_output_chars;
                        self.native_verify = Some(crate::native::VerifyTask::spawn(
                            root.clone(),
                            name.clone(),
                            spec,
                            max_chars,
                        ));
                        self.state.ai.busy = true;
                        self.state
                            .ai
                            .entries
                            .push(format!("[verify] {name} after edit"));
                        return;
                    }
                }
                if (call.tool == "edit_file" || call.tool == "write_file")
                    && let Some(raw) = call.arguments.get("path").and_then(Value::as_str)
                    && let Ok(path) = crate::native::resolve_path(&root, raw)
                    && self.state.documents.values().any(|document| {
                        document
                            .path
                            .as_deref()
                            .is_some_and(|open| super::same_path(open, &path))
                    })
                    && self.lsp.as_ref().is_some_and(|lsp| lsp.supports(&path))
                {
                    self.native_diagnostics = Some(super::DiagnosticWait {
                        path,
                        prefix: text,
                        started: std::time::Instant::now(),
                    });
                    self.state
                        .ai
                        .entries
                        .push("[check] LSP diagnostics...".to_string());
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

    pub(crate) fn finish_native_diagnostics(
        &mut self,
        path: std::path::PathBuf,
        diagnostics: Vec<crate::lsp::Diagnostic>,
    ) {
        let Some(wait) = self.native_diagnostics.as_ref() else {
            return;
        };
        if !super::same_path(&wait.path, &path) {
            return;
        }
        let wait = self.native_diagnostics.take().expect("checked");
        let Some(waiting) = self.state.ai.waiting.take() else {
            return;
        };
        let tool = waiting.tool.clone();
        let errors = diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::lsp::Severity::Error);
        let text = format!(
            "{}\n\nLSP diagnostics:\n{}",
            wait.prefix,
            format_diagnostics(&diagnostics)
        );
        self.state.ai.waiting = Some(waiting);
        self.finish_ai_tool(
            crate::native::SERVER,
            &tool,
            &[ToolContent::Text(text)],
            errors,
        );
        self.dirty = true;
    }

    pub(crate) fn expire_native_diagnostics(&mut self) {
        let Some(wait) = self.native_diagnostics.as_ref() else {
            return;
        };
        if wait.started.elapsed() < std::time::Duration::from_secs(5) {
            return;
        }
        let wait = self.native_diagnostics.take().expect("checked");
        let Some(waiting) = self.state.ai.waiting.take() else {
            return;
        };
        let tool = waiting.tool.clone();
        self.state.ai.waiting = Some(waiting);
        self.finish_ai_tool(
            crate::native::SERVER,
            &tool,
            &[ToolContent::Text(format!(
                "{}\n\nLSP diagnostics: timed out",
                wait.prefix
            ))],
            false,
        );
        self.dirty = true;
    }

    pub(crate) fn pump_native_verify(&mut self) {
        let Some(task) = self.native_verify.as_ref() else {
            return;
        };
        let Some(result) = task.try_recv() else {
            return;
        };
        self.native_verify = None;
        // The result belongs to whichever tool was awaiting it: an explicit
        // `verify` call, or the edit whose automatic pass just ran.
        let Some(waiting) = self.state.ai.waiting.take() else {
            return;
        };
        let tool = waiting.tool.clone();
        self.state.ai.waiting = Some(waiting);
        self.finish_ai_tool(
            crate::native::SERVER,
            &tool,
            &[ToolContent::Text(result.output)],
            !result.success,
        );
        self.dirty = true;
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
        let tool = waiting.tool.clone();
        let text = format!("{prefix}\n\ncargo check:\n{}", result.output);
        self.state.ai.waiting = Some(waiting);
        self.finish_ai_tool(
            crate::native::SERVER,
            &tool,
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
        if call.tool == "write_file" {
            return self.run_native_write(root, call, raw);
        }
        let path = crate::native::resolve_path(root, raw)?;
        let document = self
            .state
            .documents
            .values()
            .find(|document| {
                document
                    .path
                    .as_deref()
                    .is_some_and(|open| super::same_path(open, &path))
            })
            .map(|document| (document.id, document.dirty));
        if let Some((_, true)) = document {
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
        match crate::native::edit_with_undo(root, &call.arguments) {
            Ok((text, undo)) => {
                self.native_undo.push(undo);
                if let Some((id, _)) = document {
                    self.reload_document(id)
                        .map_err(|error| format!("reload after edit: {error}"))?;
                }
                Ok(text)
            }
            Err(error) => Err(error),
        }
    }

    fn run_native_write(
        &mut self,
        root: &std::path::Path,
        call: &AiToolCall,
        raw: &str,
    ) -> Result<String, String> {
        let existing = if std::path::Path::new(raw).is_absolute() {
            std::path::Path::new(raw).exists()
        } else {
            root.join(raw).exists()
        };
        let document = if existing {
            let path = crate::native::resolve_path(root, raw)?;
            self.state
                .documents
                .values()
                .find(|document| {
                    document
                        .path
                        .as_deref()
                        .is_some_and(|open| super::same_path(open, &path))
                })
                .map(|document| (document.id, document.dirty))
        } else {
            None
        };
        if let Some((_, true)) = document {
            return Err("file has unsaved changes; save or reload it first".to_string());
        }
        match crate::native::write_with_undo(root, &call.arguments) {
            Ok((text, undo)) => {
                self.native_undo.push(undo);
                if let Some((id, _)) = document {
                    self.reload_document(id)
                        .map_err(|error| format!("reload after write: {error}"))?;
                }
                Ok(text)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn undo_ai_edit(&mut self) {
        let Some(undo) = self.native_undo.pop() else {
            self.notify("no native edit to undo");
            return;
        };
        let path = match crate::native::undo_edit(&self.state.explorer.root, &undo) {
            Ok(path) => path,
            Err(error) => {
                self.native_undo.push(undo);
                self.notify(format!("native undo refused: {error}"));
                return;
            }
        };
        if let Some(id) = self.state.documents.values().find_map(|document| {
            document
                .path
                .as_deref()
                .is_some_and(|open| super::same_path(open, &path))
                .then_some(document.id)
        }) {
            let _ = self.reload_document(id);
        }
        self.state
            .ai
            .entries
            .push(format!("[undo] restored {}", path.display()));
        self.notify("native edit undone");
        self.dirty = true;
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
        if self.background_agent {
            self.background_agent = false;
            self.objective_queue = false;
            self.objectives.queue_active = false;
            let _ = crate::objectives::save(&self.objectives);
            self.finish_objective("blocked");
            self.state
                .ai
                .entries
                .push("[background] stopped after denial".to_string());
        }
        self.state.ai.busy = false;
        self.save_ai_history();
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
        crate::logging::event(
            "tool_result",
            serde_json::json!({
                "server": server,
                "tool": tool,
                "error": is_error,
                "chars": text.chars().count(),
            }),
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
        if self.state.ai.pending.is_empty() {
            self.start_ai_request();
        } else if self.background_agent {
            self.state.ai.busy = true;
            self.auto_allow_background_tool();
        } else {
            let call = self.state.ai.pending.front().expect("pending tool");
            self.state.ai.entries.push(format!(
                "Allow {}/{} [{}]? [y] yes  [n] no",
                call.server, call.tool, call.capability
            ));
            self.state.ai.busy = false;
        }
        self.save_ai_history();
        self.dirty = true;
    }

    pub(crate) fn load_ai_history(&mut self) {
        if !self.state.config.ai.persist_history {
            return;
        }
        let Some(history) = crate::ai_history::load() else {
            return;
        };
        self.state.ai.entries = history.entries;
        self.state.ai.conversation = history.conversation;
    }

    fn save_ai_history(&self) {
        if !self.state.config.ai.persist_history {
            return;
        }
        let history = crate::ai_history::History {
            version: crate::ai_history::VERSION,
            entries: self.state.ai.entries.clone(),
            conversation: self.state.ai.conversation.clone(),
        };
        let _ = crate::ai_history::save(&history);
    }

    fn remove_ai_history(&self) {
        crate::ai_history::remove();
    }
}

fn format_diagnostics(diagnostics: &[crate::lsp::Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "(none)".to_string();
    }
    diagnostics
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                crate::lsp::Severity::Error => "error",
                crate::lsp::Severity::Warning => "warning",
                crate::lsp::Severity::Info => "info",
            };
            format!(
                "{}:{}: {severity}: {}",
                diagnostic.line + 1,
                diagnostic.column + 1,
                diagnostic.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn insert_ai_char(app: &mut App, ch: char) {
    let byte = char_byte(&app.state.ai.input, app.state.ai.cursor);
    app.state.ai.input.insert(byte, ch);
    app.state.ai.cursor += 1;
}

fn char_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_format_with_one_based_positions() {
        let diagnostics = vec![crate::lsp::Diagnostic {
            line: 2,
            column: 4,
            severity: crate::lsp::Severity::Error,
            message: "unknown name".to_string(),
        }];
        assert_eq!(format_diagnostics(&diagnostics), "3:5: error: unknown name");
        assert_eq!(format_diagnostics(&[]), "(none)");
    }
}
