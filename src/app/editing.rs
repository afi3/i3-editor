//! Document and buffer operations.

use std::path::PathBuf;

use crate::editor::document::Document;
use crate::editor::text as textutil;
use crate::error::{Error, Result};
use crate::input::PromptKind;
use crate::state::{AppState, DocumentId, Focus};

use super::App;

impl App {
    /// True when the focused document is the read-only large-file view.
    pub(crate) fn active_read_only(&self) -> bool {
        matches!(self.state.focus, Focus::Pane(_))
            && self
                .state
                .active_document()
                .is_some_and(Document::is_read_only)
    }

    /// Report why an edit was refused. Returns true when it was refused.
    pub(crate) fn refuse_read_only(&mut self) -> bool {
        if !self.active_read_only() {
            return false;
        }
        let limit = crate::editor::document::MAX_FILE_BYTES >> 20;
        let name = self
            .state
            .active_document()
            .and_then(|document| document.path.as_deref())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "document".to_string());
        self.notify(format!("{name} is read-only (larger than {limit} MiB)"));
        true
    }

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
            let mut prompt = crate::input::PromptState::new(PromptKind::QuitConfirm);
            prompt.set_candidates(vec!["Quit".to_string(), "Cancel".to_string()]);
            self.state.mode = crate::state::InputMode::Prompt(prompt);
            self.notify("quit? Enter confirms, Esc cancels");
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
        candidates.push("Cancel".to_string());
        let mut prompt = crate::input::PromptState::new(PromptKind::QuitDirty);
        prompt.set_candidates(candidates);
        self.state.mode = crate::state::InputMode::Prompt(prompt);
        self.notify(format!(
            "{} modified document(s): save, discard, or cancel",
            dirty.len()
        ));
        Ok(())
    }

    /// Save every modified document that has a path. Returns false when a save
    /// was deferred to the external-change prompt, so the caller must not quit.
    pub(crate) fn save_all(&mut self) -> Result<bool> {
        let ids: Vec<DocumentId> = self
            .state
            .documents
            .iter()
            .filter(|(_, document)| document.dirty)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let Some(path) = self
                .state
                .documents
                .get(&id)
                .and_then(|document| document.path.clone())
            else {
                self.save_document(id)?;
                continue;
            };
            if !self.save_document_to(id, path, false)? {
                self.apply_document_indents();
                return Ok(false);
            }
        }
        self.apply_document_indents();
        Ok(true)
    }

    pub(crate) fn new_scratch(&mut self) -> Result<()> {
        let id = self.state.ids.document();
        let mut document = Document::empty(id, None);
        document.indent = self.base_indent();
        self.state.documents.insert(id, document);
        self.activate_document(id)?;
        self.notify("new scratch buffer");
        self.plugin_event("open");
        Ok(())
    }

    pub(crate) fn save_document(&mut self, id: DocumentId) -> Result<()> {
        let document = self
            .state
            .documents
            .get_mut(&id)
            .ok_or(Error::UnknownDocument(id))?;
        document.save()
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
        let id = self
            .state
            .active_document_id()
            .ok_or(Error::NoActiveDocument)?;
        self.save_document_to(id, path, false)?;
        self.apply_document_indents();
        Ok(())
    }

    /// Save `id` to `path`. Returns false when the external-change prompt took
    /// over instead of writing. `force` skips the disk check.
    fn save_document_to(&mut self, id: DocumentId, path: PathBuf, force: bool) -> Result<bool> {
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
                return Ok(false);
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
        self.refresh_git_marks();
        self.plugin_event("save");
        Ok(true)
    }

    pub(crate) fn force_save_document(&mut self, id: DocumentId, path: PathBuf) -> Result<()> {
        self.save_document_to(id, path, true)?;
        self.apply_document_indents();
        Ok(())
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

    pub(crate) fn close_document(&mut self) -> Result<()> {
        let pane_id = self.focused_or_first_pane()?;
        let Some(pane) = self.state.panes.get(&pane_id) else {
            return Err(Error::UnknownPane(pane_id));
        };
        let Some(id) = pane.active else {
            return Ok(());
        };
        self.close_document_id(id)
    }

    pub(crate) fn close_document_id(&mut self, id: DocumentId) -> Result<()> {
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
        let mut revealed = Vec::new();
        for (pane_id, pane) in self.state.panes.iter_mut() {
            pane.tabs.retain(|tab| *tab != id);
            if pane.active == Some(id) {
                pane.active = pane.tabs.first().copied();
                pane.cursor = 0;
                pane.anchor = None;
                pane.scroll_top = 0;
                pane.scroll_left = 0;
                revealed.push((*pane_id, pane.active));
            }
        }
        for (pane_id, active) in revealed {
            if let Some(active) = active {
                self.reveal_document_tab(pane_id, active);
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
        self.reveal_document_tab(pane_id, id);
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
            pane.extra.clear();
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
            pane.extra.clear();
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
            .map(|pane| !pane.selected_ranges().is_empty())
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
            let ranges = pane.selected_ranges();
            let Some(id) = pane.active else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&id) else {
                return Ok(());
            };
            let mut parts = Vec::with_capacity(ranges.len());
            for range in &ranges {
                parts.push(document.rope().slice(range.clone()).to_string());
            }
            let copied = parts.join("\n");
            *clipboard = copied.clone();
            if cut {
                let cursor_before = pane.cursor;
                let scroll = (pane.scroll_top, pane.scroll_left);
                let cursor_after = crate::state::cursors_after(&ranges, 0)
                    .last()
                    .copied()
                    .unwrap_or(cursor_before);
                document.replace_ranges(&ranges, "", cursor_before, cursor_after, scroll);
                pane.apply_edit_result(&ranges, 0);
            }
            copied
        };
        let action = if cut { "cut" } else { "copied" };
        match crate::clipboard::copy_osc52(&copied) {
            Some(reason) => self.notify(format!("{action}: internal clipboard only ({reason})")),
            None => self.notify(action),
        }
        Ok(())
    }

    /// Insert one typed character, pairing brackets and quotes. Paste and
    /// command-driven insertions keep using `insert_text` unchanged.
    pub(crate) fn insert_typed_char(&mut self, ch: char) -> Result<()> {
        if self
            .state
            .focused_pane()
            .is_some_and(|pane| !pane.extra.is_empty())
        {
            return self.insert_text(&ch.to_string());
        }
        self.insert_paired_char(ch)
    }

    fn insert_paired_char(&mut self, ch: char) -> Result<()> {
        let language = self
            .state
            .active_document()
            .map(|document| crate::syntax::Language::from_path(document.path.as_deref()))
            .unwrap_or_default();
        if ch == '`' && language == crate::syntax::Language::Markdown {
            return self.insert_text("`");
        }
        let closer = match ch {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            '`' => Some('`'),
            _ => None,
        };
        if let Some(closer) = closer {
            let selection = self.state.focused_pane().and_then(|pane| pane.selection());
            if selection.is_none() {
                let next = self
                    .state
                    .focused_pane()
                    .zip(self.state.active_document())
                    .map(|(pane, document)| {
                        document
                            .rope()
                            .get_char(pane.cursor)
                            .map(|next| next == closer)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if next {
                    return self.insert_text(&ch.to_string());
                }
                self.insert_text(&format!("{ch}{closer}"))?;
                if let Some(pane) = self.state.focused_pane_mut() {
                    pane.cursor = pane.cursor.saturating_sub(1);
                    pane.target_col = None;
                }
                return Ok(());
            }
        }
        if matches!(ch, ')' | ']' | '}' | '"' | '\'' | '`') {
            let skip = self
                .state
                .focused_pane()
                .zip(self.state.active_document())
                .is_some_and(|(pane, document)| {
                    document
                        .rope()
                        .get_char(pane.cursor)
                        .is_some_and(|next| next == ch)
                });
            if skip {
                return self.move_cursor(crate::command::Move::Right, false);
            }
        }
        self.insert_text(&ch.to_string())
    }

    pub(crate) fn convert_line_endings(
        &mut self,
        target: crate::editor::document::Eol,
    ) -> Result<()> {
        if self.refuse_read_only() {
            return Ok(());
        }
        let message = {
            let Some((pane, document)) = self.state.pane_and_document() else {
                return Ok(());
            };
            let changed = document.convert_eol(target);
            pane.cursor = pane.cursor.min(document.len_chars());
            pane.anchor = None;
            pane.extra.clear();
            if changed {
                format!("line endings: {}", target.as_str())
            } else {
                format!("already {}", target.as_str())
            }
        };
        self.notify(message);
        self.ensure_cursor_visible();
        self.refresh_git_marks();
        self.plugin_event("change");
        Ok(())
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() || self.refuse_read_only() {
            return Ok(());
        }
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
            let ranges = pane.edit_ranges();
            let inserted = text.chars().count();
            let cursor_after = crate::state::cursors_after(&ranges, inserted)
                .last()
                .copied()
                .unwrap_or(pane.cursor);
            document.replace_ranges(&ranges, text, pane.cursor, cursor_after, scroll_before);
            pane.apply_edit_result(&ranges, inserted);
            if text.contains('\n') || text.contains('\r') || ranges.len() > 1 {
                document.history.break_run();
            }
        }
        self.ensure_cursor_visible();
        self.plugin_event("change");
        Ok(())
    }

    /// `Tab` inserts spaces up to the next tab stop, or a literal tab when the
    /// document's `insert_spaces` setting is off.
    pub(crate) fn insert_tab(&mut self) -> Result<()> {
        let (insert_spaces, spaces) = {
            let Some((pane, document)) = self.state.pane_and_document() else {
                return Ok(());
            };
            let indent = document.indent;
            if !indent.insert_spaces {
                (false, 0)
            } else {
                let tab_size = indent.tab_width.max(1);
                let line = document.line_of_char(pane.cursor);
                let line_start = document.char_of_line(line);
                let text = document.line_text(line);
                let column =
                    textutil::cells_prefix(&text, pane.cursor.saturating_sub(line_start), tab_size);
                let tab = u16::try_from(tab_size).unwrap_or(u16::MAX);
                (true, (tab - (column % tab)) as usize)
            }
        };
        if !insert_spaces {
            return self.insert_text("\t");
        }
        self.insert_text(&" ".repeat(spaces))
    }

    pub(crate) fn insert_newline(&mut self) -> Result<()> {
        let auto_indent = self.state.config.editor.auto_indent;
        let (indent, eol) = {
            let Some((pane, document)) = self.state.pane_and_document() else {
                return Ok(());
            };
            let indent = if !auto_indent {
                String::new()
            } else {
                let line = document.line_of_char(pane.cursor);
                let text = document.line_text(line);
                text.chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .collect::<String>()
            };
            (indent, document.line_ending())
        };
        self.insert_text(&format!("{eol}{indent}"))
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
        if self.refuse_read_only() {
            return;
        }
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
            let mut ranges = Vec::new();
            for range in pane.edit_ranges() {
                if range.start < range.end {
                    ranges.push(range);
                } else if backward {
                    let start = textutil::prev_grapheme(document.rope(), range.start);
                    if start < range.start {
                        ranges.push(start..range.start);
                    }
                } else {
                    let end = textutil::next_grapheme(document.rope(), range.start);
                    if end > range.start {
                        ranges.push(range.start..end);
                    }
                }
            }
            ranges.sort_by_key(|range| (range.start, range.end));
            let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
            for range in ranges {
                match merged.last_mut() {
                    Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                    _ => merged.push(range),
                }
            }
            if merged.is_empty() {
                return;
            }
            let cursor_after = crate::state::cursors_after(&merged, 0)
                .last()
                .copied()
                .unwrap_or(cursor_before);
            document.replace_ranges(&merged, "", cursor_before, cursor_after, scroll);
            pane.apply_edit_result(&merged, 0);
        }
        self.ensure_cursor_visible();
        self.plugin_event("change");
    }
}
