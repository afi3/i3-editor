//! One-shot AI chat prompt and read-only response overlay.

use crate::input::{KeyCombo, PromptKind, PromptState};
use crate::state::{HelpView, InputMode};

use super::App;

impl App {
    pub(crate) fn open_ai(&mut self) {
        self.state.mode = InputMode::Prompt(PromptState::new(PromptKind::AiChat));
        self.state.ai_view = None;
        self.state.mcp_view = None;
        self.dirty = true;
    }

    pub(crate) fn submit_ai(&mut self, prompt: PromptState) {
        let question = prompt.fields.first().cloned().unwrap_or_default();
        if question.trim().is_empty() {
            self.notify("AI prompt is empty");
            return;
        }
        if self.ai_task.is_some() {
            self.notify("an AI request is already running");
            return;
        }
        let config = self.state.config.ai.clone();
        self.ai_task = Some(crate::ai::spawn(config, question));
        self.notify("asking the configured model");
        self.dirty = true;
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
        let (lines, message) = match result {
            Ok(reply) => {
                let mut lines = vec!["Esc or q closes this overlay".to_string(), String::new()];
                lines.extend(reply.lines().map(str::to_string));
                (lines, "AI response ready".to_string())
            }
            Err(error) => (
                vec![
                    "AI request failed".to_string(),
                    String::new(),
                    error.clone(),
                ],
                error,
            ),
        };
        self.state.ai_view = Some(HelpView { lines, scroll: 0 });
        self.state.completions = None;
        self.state.hover = None;
        self.notify(message);
        self.dirty = true;
    }

    pub(crate) fn handle_ai_key(&mut self, combo: KeyCombo) -> bool {
        let Some(view) = self.state.ai_view.as_mut() else {
            return false;
        };
        if super::help::read_only_key(view, combo) {
            self.state.ai_view = None;
        }
        true
    }
}
