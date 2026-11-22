//! Crash-recovery integration: swap writes, replay, and the startup prompt.

use std::time::{Duration, Instant};

use crate::input::{PromptKind, PromptState};
use crate::state::{DocumentId, InputMode};

use super::App;

impl App {
    /// Replay swaps left by a crash and queue the recovery prompt.
    pub(crate) fn load_recoveries(&mut self) {
        let Some(dir) = crate::recovery::default_dir() else {
            return;
        };
        self.swap_dir = Some(dir.clone());
        let recovered =
            crate::recovery::recover_from(&dir, &mut self.state.documents, &mut self.state.ids);
        if recovered.is_empty() {
            return;
        }
        for id in &recovered {
            if !self.state.is_document_referenced(*id)
                && let Some(pane) = self.state.focused_pane_mut()
            {
                pane.tabs.push(*id);
                if pane.active.is_none() {
                    pane.active = Some(*id);
                }
            }
        }
        self.state
            .warnings
            .push(format!("{} unsaved buffer(s) recovered", recovered.len()));
        self.recoveries = recovered;
        self.begin_recovery_prompt();
    }

    /// Write every dirty buffer and drop swaps for clean ones.
    pub(crate) fn flush_swaps(&mut self) {
        let Some(dir) = self.swap_dir.clone() else {
            return;
        };
        if let Err(error) = crate::recovery::write_to(&dir, self.state.documents.values()) {
            self.notify(format!("swap not written: {error}"));
        }
    }

    /// One swap pass per second; called from the event loop.
    pub(crate) fn autosave_swaps(&mut self) {
        if self.last_swap.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_swap = Instant::now();
        self.flush_swaps();
    }

    /// Show the first unresolved recovery, if any.
    pub(crate) fn begin_recovery_prompt(&mut self) {
        while let Some(id) = self.recoveries.first().copied() {
            let Some(document) = self.state.documents.get(&id) else {
                self.recoveries.remove(0);
                continue;
            };
            let name = document.display_name();
            let mut prompt = PromptState::new(PromptKind::Recovery);
            prompt.document = Some(id);
            prompt.set_candidates(vec![
                format!("Save recovered buffer: {name}"),
                format!("Reload from disk: {name}"),
                format!("Keep unsaved buffer: {name}"),
            ]);
            self.state.mode = InputMode::Prompt(prompt);
            return;
        }
    }

    pub(crate) fn resolve_recovery(
        &mut self,
        id: DocumentId,
        choice: &str,
    ) -> crate::error::Result<()> {
        if choice.starts_with("Save") {
            self.save_document(id)?;
        } else if choice.starts_with("Reload") {
            self.reload_document(id)?;
        }
        let path = self
            .state
            .documents
            .get(&id)
            .and_then(|document| document.path.clone());
        if let (Some(dir), Some(path)) = (&self.swap_dir, path) {
            crate::recovery::clear(dir, &path);
        }
        self.recoveries.retain(|recovered| *recovered != id);
        self.begin_recovery_prompt();
        Ok(())
    }
}
