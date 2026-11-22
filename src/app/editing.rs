//! Document and buffer operations.

use std::path::PathBuf;

use crate::editor::document::Document;
use crate::editor::text as textutil;
use crate::error::{Error, Result};
use crate::input::PromptKind;
use crate::state::{AppState, DocumentId, Focus};

use super::App;

impl App {
    /// Quit through a confirmation while any document is modified.
    pub(crate) fn request_quit(&mut self) -> Result<()> {
        let dirty: Vec<DocumentId> = self
            .state
            .documents
            .iter()
            .filter(|(_, document)| document.dirty)
            .map(|(id, _)| *id)
            .collect();
        if dirty.is_empty() {
            self.state.should_quit = true;
            return Ok(());
        }
        let all_saved = dirty
            .iter()
            .all(|id| self.state.documents[id].path.is_some());
        let mut candidates = Vec::new();
        if all_saved {
            candidates.push("Save all and quit".to_string());
        }
        candidates.push("Discard all and quit".to_string());
        let mut prompt = crate::input::PromptState::new(PromptKind::QuitDirty);
        prompt.set_candidates(candidates);
        self.state.mode = crate::state::InputMode::Prompt(prompt);
        self.notify(format!(
            "{} modified document(s): save all or discard all",
            dirty.len()
        ));
        Ok(())
    }

    /// Save every modified document that has a path.
    pub(crate) fn save_all(&mut self) -> Result<()> {
        let ids: Vec<DocumentId> = self
            .state
            .documents
            .iter()
            .filter(|(_, document)| document.dirty)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let document = self
                .state
                .documents
                .get_mut(&id)
                .ok_or(Error::UnknownDocument(id))?;
            document.save()?;
        }
        Ok(())
    }

    pub(crate) fn save(&mut self) -> Result<()> {
        let Some(document) = self.state.active_document() else {
            return Err(Error::NoActiveDocument);
        };
        match document.path.clone() {
            Some(path) => self.save_to(path),
            None => {
                self.open_prompt(PromptKind::SaveAs);
                Ok(())
            }
        }
    }

    pub(crate) fn save_to(&mut self, path: PathBuf) -> Result<()> {
        self.save_to_inner(path, false)
    }

    fn save_to_inner(&mut self, path: PathBuf, force: bool) -> Result<()> {
        let id = self
            .state
            .active_document_id()
            .ok_or(Error::NoActiveDocument)?;
        let same_path = {
            let document = self
                .state
                .documents
                .get(&id)
                .ok_or(Error::UnknownDocument(id))?;
            document.path.as_deref() == Some(path.as_path())
        };
        if same_path && !force {
            let changed = self
                .state
                .documents
                .get(&id)
                .is_some_and(Document::disk_changed);
            if changed {
                self.open_external_change_prompt(id, path);
                return Ok(());
            }
        }
        let document = self
            .state
            .documents
            .get_mut(&id)
            .ok_or(Error::UnknownDocument(id))?;
        if document.path.as_deref() == Some(path.as_path()) {
            document.save()?;
        } else {
            document.save_as(path.clone())?;
        }
        self.notify(format!("saved {}", path.display()));
        self.state.explorer.refresh();
        Ok(())
    }

    pub(crate) fn force_save_to(&mut self, path: PathBuf) -> Result<()> {
        self.save_to_inner(path, true)
    }

    /// Ask what to do when the file changed under a modified buffer.
    pub(crate) fn open_external_change_prompt(&mut self, id: DocumentId, path: PathBuf) {
        self.open_prompt(PromptKind::ExternalChange);
        if let crate::state::InputMode::Prompt(prompt) = &mut self.state.mode {
            prompt.set_candidates(vec![
                "Overwrite with the buffer".to_string(),
                "Reload from disk".to_string(),
            ]);
            prompt.target = Some(path);
            prompt.document = Some(id);
        }
    }

    /// Replace a buffer with the file on disk and clear its undo history.
    pub(crate) fn reload_document(&mut self, id: DocumentId) -> Result<()> {
        let (path, len) = {
            let document = self
                .state
                .documents
                .get_mut(&id)
                .ok_or(Error::UnknownDocument(id))?;
            document.reload()?;
            (
                document.path.clone().unwrap_or_default(),
                document.len_chars(),
            )
        };
        for pane in self.state.panes.values_mut() {
            if pane.tabs.contains(&id) {
                pane.cursor = pane.cursor.min(len);
                pane.anchor = None;
                pane.target_col = None;
            }
        }
        self.ensure_cursor_visible();
        self.notify(format!("reloaded {}", path.display()));
        Ok(())
    }

    pub(crate) fn new_buffer(&mut self) -> Result<()> {
        let id = self.state.ids.document();
        self.state.documents.insert(id, Document::empty(id, None));
        self.activate_document(id)?;
        self.notify("new buffer");
        Ok(())
    }

    pub(crate) fn close_document(&mut self) -> Result<()> {
        let pane_id = self.focused_or_first_pane()?;
        let Some(pane) = self.state.panes.get(&pane_id) else {
            return Err(Error::UnknownPane(pane_id));
        };
        let Some(id) = pane.active else {
            return Ok(());
        };
        let dirty = self
            .state
            .documents
            .get(&id)
            .map(|document| document.dirty)
            .unwrap_or(false);
        if dirty {
            let mut prompt = crate::input::PromptState::new(PromptKind::Discard);
            prompt.document = Some(id);
            self.state.mode = crate::state::InputMode::Prompt(prompt);
            return Ok(());
        }
        self.force_close_document(id);
        Ok(())
    }

    pub(crate) fn force_close_document(&mut self, id: DocumentId) {
        for pane in self.state.panes.values_mut() {
            pane.tabs.retain(|tab| *tab != id);
            if pane.active == Some(id) {
                pane.active = pane.tabs.first().copied();
                pane.cursor = 0;
                pane.anchor = None;
                pane.scroll_top = 0;
                pane.scroll_left = 0;
            }
        }
        if !self.state.is_document_referenced(id) {
            self.state.documents.remove(&id);
        }
    }

    pub(crate) fn toggle_explorer(&mut self) {
        self.state.explorer.visible = !self.state.explorer.visible;
        if self.state.explorer.visible {
            self.state.focus = Focus::Explorer;
        } else if matches!(self.state.focus, Focus::Explorer)
            && let Ok(id) = self.focused_or_first_pane()
        {
            self.state.focus = Focus::Pane(id);
        }
    }

    pub(crate) fn explorer_open(&mut self) -> Result<()> {
        let Some(row) = self.state.explorer.selected_row().cloned() else {
            return Ok(());
        };
        if row.is_dir {
            self.state.explorer.toggle_selected();
            return Ok(());
        }
        self.open_document(row.path)
    }

    pub(crate) fn cycle_document(&mut self, delta: isize) -> Result<()> {
        let pane_id = self.focused_or_first_pane()?;
        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let pane = panes.get_mut(&pane_id).ok_or(Error::UnknownPane(pane_id))?;
        if pane.tabs.is_empty() {
            return Ok(());
        }
        let current = pane
            .tabs
            .iter()
            .position(|id| Some(*id) == pane.active)
            .unwrap_or(0) as isize;
        let len = pane.tabs.len() as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        let id = pane.tabs[next];
        pane.active = Some(id);
        pane.anchor = None;
        if let Some(document) = documents.get(&id) {
            pane.cursor = pane.cursor.min(document.len_chars());
        }
        Ok(())
    }

    pub(crate) fn undo(&mut self) -> Result<()> {
        let pane_id = match self.state.focus {
            Focus::Pane(id) => id,
            Focus::Explorer | Focus::Shell => return Ok(()),
        };
        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let Some(pane) = panes.get_mut(&pane_id) else {
            return Ok(());
        };
        let Some(id) = pane.active else {
            return Ok(());
        };
        let Some(document) = documents.get_mut(&id) else {
            return Ok(());
        };
        if let Some((cursor, (top, left))) = document.undo() {
            pane.cursor = cursor.min(document.len_chars());
            pane.anchor = None;
            pane.scroll_top = top;
            pane.scroll_left = left;
        }
        Ok(())
    }

    pub(crate) fn redo(&mut self) -> Result<()> {
        let pane_id = match self.state.focus {
            Focus::Pane(id) => id,
            Focus::Explorer | Focus::Shell => return Ok(()),
        };
        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let Some(pane) = panes.get_mut(&pane_id) else {
            return Ok(());
        };
        let Some(id) = pane.active else {
            return Ok(());
        };
        let Some(document) = documents.get_mut(&id) else {
            return Ok(());
        };
        if let Some((cursor, (top, left))) = document.redo() {
            pane.cursor = cursor.min(document.len_chars());
            pane.anchor = None;
            pane.scroll_top = top;
            pane.scroll_left = left;
        }
        Ok(())
    }

    pub(crate) fn select_all(&mut self) -> Result<()> {
        let pane_id = match self.state.focus {
            Focus::Pane(id) => id,
            Focus::Explorer | Focus::Shell => return Ok(()),
        };
        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let Some(pane) = panes.get_mut(&pane_id) else {
            return Ok(());
        };
        let Some(id) = pane.active else {
            return Ok(());
        };
        let Some(document) = documents.get(&id) else {
            return Ok(());
        };
        pane.select_all(document);
        Ok(())
    }

    pub(crate) fn copy_selection(&mut self, cut: bool) -> Result<()> {
        let has_selection = self
            .state
            .focused_pane()
            .map(|pane| pane.selection().is_some())
            .unwrap_or(false);
        if !has_selection {
            self.notify("nothing to copy");
            return Ok(());
        }
        let copied = {
            let AppState {
                documents,
                panes,
                clipboard,
                focus,
                ..
            } = &mut self.state;
            let pane_id = match focus {
                Focus::Pane(id) => *id,
                Focus::Explorer | Focus::Shell => return Ok(()),
            };
            let Some(pane) = panes.get_mut(&pane_id) else {
                return Ok(());
            };
            let Some(range) = pane.selection() else {
                return Ok(());
            };
            let Some(id) = pane.active else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&id) else {
                return Ok(());
            };
            *clipboard = document.rope.slice(range.clone()).to_string();
            if cut {
                let cursor_before = pane.cursor;
                let scroll = (pane.scroll_top, pane.scroll_left);
                document.remove(range.clone(), cursor_before, scroll);
                pane.cursor = range.start;
                pane.anchor = None;
            }
            clipboard.clone()
        };
        crate::clipboard::copy_osc52(&copied);
        self.notify(if cut { "cut" } else { "copied" });
        Ok(())
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> Result<()> {
        {
            let AppState {
                documents,
                panes,
                focus,
                ..
            } = &mut self.state;
            let pane_id = match focus {
                Focus::Pane(id) => *id,
                Focus::Explorer | Focus::Shell => return Ok(()),
            };
            let Some(pane) = panes.get_mut(&pane_id) else {
                return Ok(());
            };
            let Some(id) = pane.active else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&id) else {
                return Ok(());
            };
            let scroll_before = (pane.scroll_top, pane.scroll_left);
            if let Some(range) = pane.selection() {
                document.remove(range.clone(), pane.cursor, scroll_before);
                pane.cursor = range.start;
                pane.anchor = None;
            }
            document.insert(pane.cursor, text, pane.cursor, scroll_before);
            pane.cursor += text.chars().count();
            if text.contains('\n') {
                document.history.break_run();
            }
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    /// `Tab` inserts spaces up to the next tab stop, or a literal tab when
    /// `editor.insert_spaces` is off.
    pub(crate) fn insert_tab(&mut self) -> Result<()> {
        if !self.state.config.editor.insert_spaces {
            return self.insert_text("\t");
        }
        let tab_size = self.state.config.editor.tab_size.max(1);
        let spaces = {
            let Some((pane, document)) = self.state.pane_and_document() else {
                return Ok(());
            };
            let line = document.line_of_char(pane.cursor);
            let line_start = document.char_of_line(line);
            let text = document.line_text(line);
            let column =
                textutil::cells_prefix(&text, pane.cursor.saturating_sub(line_start), tab_size);
            let tab = u16::try_from(tab_size).unwrap_or(u16::MAX);
            (tab - (column % tab)) as usize
        };
        self.insert_text(&" ".repeat(spaces))
    }

    pub(crate) fn insert_newline(&mut self) -> Result<()> {
        let auto_indent = self.state.config.editor.auto_indent;
        let indent = {
            let Some((pane, document)) = self.state.pane_and_document() else {
                return Ok(());
            };
            if !auto_indent {
                String::new()
            } else {
                let line = document.line_of_char(pane.cursor);
                let text = document.line_text(line);
                text.chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .collect::<String>()
            }
        };
        self.insert_text(&format!("\n{indent}"))
    }

    pub(crate) fn delete_backward(&mut self) -> Result<()> {
        self.delete_with(true);
        Ok(())
    }

    pub(crate) fn delete_forward(&mut self) -> Result<()> {
        self.delete_with(false);
        Ok(())
    }

    fn delete_with(&mut self, backward: bool) {
        {
            let AppState {
                documents,
                panes,
                focus,
                ..
            } = &mut self.state;
            let pane_id = match focus {
                Focus::Pane(id) => *id,
                Focus::Explorer | Focus::Shell => return,
            };
            let Some(pane) = panes.get_mut(&pane_id) else {
                return;
            };
            let Some(id) = pane.active else {
                return;
            };
            let Some(document) = documents.get_mut(&id) else {
                return;
            };
            let scroll = (pane.scroll_top, pane.scroll_left);
            let cursor_before = pane.cursor;
            if let Some(range) = pane.selection() {
                document.remove(range.clone(), cursor_before, scroll);
                pane.cursor = range.start;
            } else {
                let range = if backward {
                    let start = textutil::prev_grapheme(&document.rope, pane.cursor);
                    start..pane.cursor
                } else {
                    pane.cursor..textutil::next_grapheme(&document.rope, pane.cursor)
                };
                if range.start < range.end {
                    document.remove(range.clone(), cursor_before, scroll);
                    pane.cursor = range.start;
                }
            }
            pane.anchor = None;
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
    }
}
