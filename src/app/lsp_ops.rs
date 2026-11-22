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
        let open: std::collections::HashSet<DocumentId> =
            self.state.documents.keys().copied().collect();
        self.state.lsp_diagnostics.retain(|id, _| open.contains(id));
        let mut pending_definition = None;
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
                        self.state.lsp_diagnostics.insert(id, diagnostics);
                    }
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
        self.notify("no language servers configured ([lsp.<name>] in config.toml)");
        false
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
