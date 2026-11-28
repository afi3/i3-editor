//! Crash-recovery integration: swap writes, replay, and the startup prompt.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::editor::document::Document;
use crate::input::{PromptKind, PromptState};
use crate::state::{DocumentId, InputMode};

use super::App;

impl App {
    /// Claim this process's swap directory and replay swaps left by instances
    /// that are gone.
    pub(crate) fn load_recoveries(&mut self) {
        let Some(base) = crate::recovery::default_dir() else {
            return;
        };
        let instance = match crate::recovery::Instance::claim(&base) {
            Ok(instance) => instance,
            Err(error) => {
                // `notify` would be overwritten by the startup message; a
                // warning is what survives into the status line.
                self.state
                    .warnings
                    .push(format!("swap directory unavailable: {error}"));
                return;
            }
        };
        let dir = instance.dir().to_path_buf();
        self.swap = Some(instance);
        let recovered = crate::recovery::recover_from(
            &base,
            &dir,
            &mut self.state.documents,
            &mut self.state.ids,
        );
        if recovered.is_empty() {
            return;
        }
        // `autosave_swaps` rewrites these swaps within a second, which would
        // hide that the file was already newer than the crash copy.
        self.recovery_stale.clear();
        for id in &recovered {
            let stale = self
                .state
                .documents
                .get(id)
                .and_then(|document| document.path.as_deref())
                .is_some_and(|path| crate::recovery::swap_is_stale(&dir, path));
            if stale {
                self.recovery_stale.insert(*id);
            }
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
        let Some(dir) = self
            .swap
            .as_ref()
            .map(|instance| instance.dir().to_path_buf())
        else {
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
        let path = self
            .state
            .documents
            .get(&id)
            .and_then(|document| document.path.clone());
        if choice.starts_with("Save") {
            if let Some(path) = &path
                && self.recovered_file_changed(id, path)
            {
                self.open_external_change_prompt(id, path.clone());
                return Ok(());
            }
            self.save_document(id)?;
        } else if choice.starts_with("Reload") {
            self.reload_document(id)?;
        }
        if let Some(path) = path
            && let Some(dir) = self
                .swap
                .as_ref()
                .map(|instance| instance.dir().to_path_buf())
        {
            crate::recovery::clear(&dir, &path);
        }
        self.recoveries.retain(|recovered| *recovered != id);
        self.begin_recovery_prompt();
        Ok(())
    }

    /// A recovered buffer must not overwrite a file that changed after its
    /// swap was written, nor one whose recorded disk state no longer matches.
    fn recovered_file_changed(&self, id: DocumentId, _path: &Path) -> bool {
        let changed = self
            .state
            .documents
            .get(&id)
            .is_some_and(Document::disk_changed);
        // The verdict is the one taken at load: the live swap is rewritten by
        // autosave and no longer reflects the crash copy's age.
        changed || self.recovery_stale.contains(&id)
    }
}
