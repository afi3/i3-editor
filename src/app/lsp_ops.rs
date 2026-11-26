//! LSP integration: snapping server events into UI state and dispatching the
//! completion, hover, and definition commands.

use crate::error::Result;
use crate::lsp::{Event, Location};
use crate::state::{CompletionPopup, DocumentId};

use super::{App, same_path};

impl App {
    /// Drop LSP state and start servers again for the current root/config.
    pub(crate) fn restart_lsp(&mut self) {
        if let Some(manager) = self.lsp.as_mut() {
            manager.restart(&self.state.config.lsp, self.state.explorer.root.clone());
        }
        self.state.lsp_diagnostics.clear();
        self.state.completions = None;
        self.state.hover = None;
    }

    /// Drain server messages and update diagnostics and popups.
    pub(crate) fn pump_lsp(&mut self) {
        let Some(manager) = self.lsp.as_mut() else {
            return;
        };
        let events = manager.pump(&self.state.documents);
        if !events.is_empty() {
            self.dirty = true;
        }
        let open: std::collections::HashSet<DocumentId> =
            self.state.documents.keys().copied().collect();
        self.state.lsp_diagnostics.retain(|id, _| open.contains(id));
        let mut pending_definition = None;
        let mut diagnostic_updates = Vec::new();
        for event in events {
            match event {
                Event::Diagnostics {
                    path,
                    mut diagnostics,
                } => {
                    if let Some(id) = self.document_for_path(&path) {
                        if let Some(document) = self.state.documents.get(&id) {
                            convert_diagnostic_columns(document, &mut diagnostics);
                        }
                        self.state.lsp_diagnostics.insert(id, diagnostics.clone());
                    }
                    diagnostic_updates.push((path, diagnostics));
                }
                Event::Completion(items) => {
                    let count = items.len();
                    self.state.hover = None;
                    self.state.completions = Some(CompletionPopup { items, selected: 0 });
                    if count == 0 {
                        self.notify("no completions");
                    }
                }
                Event::Hover(text) => {
                    self.state.completions = None;
                    self.state.hover = Some(text);
                }
                Event::Definition(location) => pending_definition = Some(location),
                Event::Warning(message) => self.notify(message),
            }
        }
        for (path, diagnostics) in diagnostic_updates {
            self.finish_native_diagnostics(path, diagnostics);
        }
        self.expire_native_diagnostics();
        if let Some(location) = pending_definition {
            let result = match location {
                Some(location) => self.goto_lsp_location(location),
                None => {
                    self.notify("no definition found");
                    Ok(())
                }
            };
            self.finish(result);
        }
    }

    fn require_lsp(&mut self) -> bool {
        if self
            .lsp
            .as_ref()
            .is_some_and(|manager| manager.is_configured())
        {
            return true;
        }
        self.notify("no language servers configured; F1 > lsp_install");
        false
    }

    pub(crate) fn lsp_install(&mut self) -> Result<()> {
        if self.lsp_install.is_some() {
            self.notify("a language server install is already running");
            return Ok(());
        }
        let Some(document) = self.state.active_document() else {
            self.notify("open a file before installing its language server");
            return Ok(());
        };
        let Some(path) = document.path.as_deref() else {
            self.notify("save the document before installing its language server");
            return Ok(());
        };
        let Some(provider) = crate::lsp_install::provider_for_path(path) else {
            self.notify(format!(
                "no installable language server for {}",
                document.display_name()
            ));
            return Ok(());
        };
        if self.state.config.lsp.servers.contains_key(provider.id) {
            self.notify(format!(
                "{} language server is already configured",
                provider.label
            ));
            return Ok(());
        }
        let Some(directory) = crate::config::lsp_dir() else {
            self.notify("no config directory for language servers");
            return Ok(());
        };
        self.lsp_install = Some(crate::lsp_install::spawn(provider, directory));
        self.notify(format!(
            "installing {} language server in the background",
            provider.label
        ));
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn lsp_uninstall(&mut self) -> Result<()> {
        if self.lsp_install.is_some() {
            self.notify("a language server install is already running");
            return Ok(());
        }
        let Some(document) = self.state.active_document() else {
            self.notify("open a file before removing its language server");
            return Ok(());
        };
        let Some(path) = document.path.as_deref() else {
            self.notify("save the document before removing its language server");
            return Ok(());
        };
        let Some(provider) = crate::lsp_install::provider_for_path(path) else {
            self.notify(format!(
                "no installable language server for {}",
                document.display_name()
            ));
            return Ok(());
        };
        match crate::config::take_managed_lsp_server(provider.id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                self.notify(format!(
                    "{} was not installed by i3-editor; edit config.toml instead",
                    provider.label
                ));
                return Ok(());
            }
            Err(error) => {
                self.notify(format!("cannot remove managed LSP config: {error}"));
                return Ok(());
            }
        };
        self.reload_config();
        if self.state.config_path.is_none() {
            self.state.config.lsp.servers.remove(provider.id);
            self.restart_lsp();
        }
        if self.state.config.lsp.servers.contains_key(provider.id) {
            self.notify(format!(
                "{} managed entry removed; explicit config still active",
                provider.label
            ));
            return Ok(());
        }
        self.notify(format!(
            "{} language server unregistered; local files under lsp/ are kept",
            provider.label
        ));
        Ok(())
    }

    pub(crate) fn pump_lsp_install(&mut self) {
        let Some(task) = self.lsp_install.as_ref() else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(event) = task.rx.try_recv() {
            events.push(event);
        }
        let label = task.label;
        let mut finished = false;
        for event in events {
            match event {
                crate::lsp_install::Event::Status(message) => {
                    self.notify(message);
                    self.dirty = true;
                }
                crate::lsp_install::Event::Done { key, server } => {
                    self.state
                        .config
                        .lsp
                        .servers
                        .insert(key.clone(), server.clone());
                    let persistence = crate::config::persist_lsp_server(&key, &server);
                    self.restart_lsp();
                    match persistence {
                        Ok(()) => self.notify(format!(
                            "{label} language server installed; diagnostics will follow"
                        )),
                        Err(error) => self.notify(format!(
                            "{label} language server active, but lsp.toml was not saved: {error}"
                        )),
                    }
                    finished = true;
                    self.dirty = true;
                }
                crate::lsp_install::Event::Failed { key, message } => {
                    self.notify(format!("{key} language server install failed: {message}"));
                    finished = true;
                    self.dirty = true;
                }
            }
        }
        if finished {
            self.lsp_install = None;
        }
    }

    fn document_for_path(&self, path: &std::path::Path) -> Option<DocumentId> {
        self.state
            .documents
            .iter()
            .find(|(_, document)| {
                document
                    .path
                    .as_deref()
                    .is_some_and(|open| same_path(open, path))
            })
            .map(|(id, _)| *id)
    }

    pub(crate) fn lsp_completion(&mut self) -> Result<()> {
        if !self.require_lsp() {
            return Ok(());
        }
        let Some(document) = self.state.active_document() else {
            self.notify("no active document");
            return Ok(());
        };
        let Some(cursor) = self.state.focused_pane().map(|pane| pane.cursor) else {
            return Ok(());
        };
        let Some(manager) = self.lsp.as_mut() else {
            self.notify("language server unavailable");
            return Ok(());
        };
        match manager.completion(document, cursor) {
            Ok(()) => self.notify("completion requested"),
            Err(error) => self.notify(error),
        }
        Ok(())
    }

    pub(crate) fn lsp_hover(&mut self) -> Result<()> {
        if !self.require_lsp() {
            return Ok(());
        }
        let Some(document) = self.state.active_document() else {
            self.notify("no active document");
            return Ok(());
        };
        let Some(cursor) = self.state.focused_pane().map(|pane| pane.cursor) else {
            return Ok(());
        };
        let Some(manager) = self.lsp.as_mut() else {
            self.notify("language server unavailable");
            return Ok(());
        };
        match manager.hover(document, cursor) {
            Ok(()) => self.notify("hover requested"),
            Err(error) => self.notify(error),
        }
        Ok(())
    }

    pub(crate) fn lsp_definition(&mut self) -> Result<()> {
        if !self.require_lsp() {
            return Ok(());
        }
        let Some(document) = self.state.active_document() else {
            self.notify("no active document");
            return Ok(());
        };
        let Some(cursor) = self.state.focused_pane().map(|pane| pane.cursor) else {
            return Ok(());
        };
        let Some(manager) = self.lsp.as_mut() else {
            self.notify("language server unavailable");
            return Ok(());
        };
        match manager.definition(document, cursor) {
            Ok(()) => self.notify("definition requested"),
            Err(error) => self.notify(error),
        }
        Ok(())
    }

    fn goto_lsp_location(&mut self, location: Location) -> Result<()> {
        self.open_document(location.path.clone())?;
        let Some(id) = self.state.active_document_id() else {
            return Ok(());
        };
        let (line, cursor) = {
            let Some(document) = self.state.documents.get(&id) else {
                return Ok(());
            };
            let line = location.line.min(document.line_count().saturating_sub(1));
            let text = document.line_text(line);
            let chars = crate::lsp::utf16_to_char(&text, location.character);
            (line, document.char_of_line(line) + chars)
        };
        if let Some(document) = self.state.documents.get_mut(&id) {
            document.unfold_line(line);
        }
        if let Some(pane) = self
            .state
            .focused_pane_mut()
            .filter(|pane| pane.active == Some(id))
        {
            pane.cursor = cursor;
            pane.anchor = None;
            pane.target_col = None;
        }
        self.state.completions = None;
        self.state.hover = None;
        self.ensure_cursor_visible();
        let name = self
            .state
            .documents
            .get(&id)
            .map(|document| document.display_name())
            .unwrap_or_default();
        self.notify(format!("{name}:{}", line + 1));
        Ok(())
    }

    /// Handle a key while the completion popup is open. Returns whether the key
    /// was consumed; other keys close the popup and reach the editor.
    pub(crate) fn handle_completion_key(&mut self, combo: crate::input::KeyCombo) -> bool {
        use ratatui::crossterm::event::KeyCode;

        let Some(popup) = &mut self.state.completions else {
            return false;
        };
        match combo.code {
            KeyCode::Up => {
                popup.selected = popup.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(popup.items.len().saturating_sub(1));
            }
            KeyCode::Esc => self.state.completions = None,
            KeyCode::Enter => {
                let Some(item) = popup.items.get(popup.selected).cloned() else {
                    self.state.completions = None;
                    return true;
                };
                self.state.completions = None;
                let result = self.insert_text(&item.insert);
                self.finish(result);
            }
            _ => {
                self.state.completions = None;
                return false;
            }
        }
        true
    }

    /// Close a hover popup; any non-Esc key also closes it and continues.
    pub(crate) fn handle_hover_key(&mut self, combo: crate::input::KeyCombo) -> bool {
        if self.state.hover.is_none() {
            return false;
        }
        if combo.code == ratatui::crossterm::event::KeyCode::Esc {
            self.state.hover = None;
            return true;
        }
        self.state.hover = None;
        false
    }
}

fn convert_diagnostic_columns(
    document: &crate::editor::document::Document,
    diagnostics: &mut [crate::lsp::Diagnostic],
) {
    for diagnostic in diagnostics {
        if diagnostic.line >= document.line_count() {
            continue;
        }
        let text = document.line_text(diagnostic.line);
        diagnostic.column = crate::lsp::utf16_to_char(&text, diagnostic.column);
    }
}
