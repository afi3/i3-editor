//! Resolve merge-conflict blocks in the active document.

use crate::conflict::{self, Choice};
use crate::error::{Error, Result};
use crate::git::StatusEntry;

use super::App;

impl App {
    pub(crate) fn resolve_conflict(&mut self, choice: Choice) -> Result<()> {
        let pane_id = self.state.focused_pane_id().ok_or(Error::NoFocusedPane)?;
        let document_id = self
            .state
            .panes
            .get(&pane_id)
            .and_then(|pane| pane.active)
            .ok_or(Error::NoActiveDocument)?;
        let cursor = self.state.panes[&pane_id].cursor;

        let block = {
            let document = self
                .state
                .documents
                .get(&document_id)
                .ok_or(Error::UnknownDocument(document_id))?;
            let blocks = conflict::blocks(document);
            conflict::at_or_after(&blocks, document.line_of_char(cursor))
        };
        let Some(block) = block else {
            self.notify("no conflict at or after the cursor");
            return Ok(());
        };

        let (range, text, new_cursor) = {
            let document = self
                .state
                .documents
                .get(&document_id)
                .ok_or(Error::UnknownDocument(document_id))?;
            let start = document.char_of_line(block.start_line);
            let end = if block.end_line + 1 < document.line_count() {
                document.char_of_line(block.end_line + 1)
            } else {
                document.len_chars()
            };
            let ours_start = document.char_of_line(block.start_line + 1);
            let separator = document.char_of_line(block.separator_line);
            let theirs_start = document.char_of_line(block.separator_line + 1);
            let theirs_end = document.char_of_line(block.end_line);
            let ours = document.rope.slice(ours_start..separator).to_string();
            let theirs = document.rope.slice(theirs_start..theirs_end).to_string();
            let text = match choice {
                Choice::Ours => ours,
                Choice::Theirs => theirs,
                Choice::Both => format!("{ours}{theirs}"),
            };
            let new_cursor = start + text.chars().count();
            (start..end, text, new_cursor)
        };

        let pane = self
            .state
            .panes
            .get(&pane_id)
            .ok_or(Error::UnknownPane(pane_id))?;
        let cursor_before = pane.cursor;
        let scroll_before = (pane.scroll_top, pane.scroll_left);
        let remaining = {
            let document = self
                .state
                .documents
                .get_mut(&document_id)
                .ok_or(Error::UnknownDocument(document_id))?;
            document.replace(range, &text, cursor_before, new_cursor, scroll_before);
            conflict::blocks(document).len()
        };
        let len = self.state.documents[&document_id].len_chars();
        let pane = self
            .state
            .panes
            .get_mut(&pane_id)
            .ok_or(Error::UnknownPane(pane_id))?;
        pane.cursor = new_cursor.min(len);
        pane.anchor = None;
        pane.target_col = None;
        self.ensure_cursor_visible();

        if remaining == 0 {
            self.notify("all conflicts resolved; stage the file with git_stage");
        } else {
            self.notify(format!(
                "took {}; {remaining} conflict(s) left",
                choice.label()
            ));
        }
        Ok(())
    }

    /// Open a path from the conflict prompt, reloading a clean stale buffer so
    /// the markers are visible, then place the cursor on the first marker.
    pub(crate) fn open_conflict_entry(&mut self, entry: StatusEntry) -> Result<()> {
        self.open_document(entry.path.clone())?;
        let id = self
            .state
            .active_document_id()
            .ok_or(Error::NoActiveDocument)?;
        let reload = {
            let document = self
                .state
                .documents
                .get(&id)
                .ok_or(Error::UnknownDocument(id))?;
            !document.dirty && document.disk_changed()
        };
        if reload {
            self.reload_document(id)?;
        }
        let line = {
            let document = self
                .state
                .documents
                .get(&id)
                .ok_or(Error::UnknownDocument(id))?;
            conflict::blocks(document)
                .first()
                .map(|block| block.start_line)
        };
        let Some(line) = line else {
            self.notify(format!("no conflict markers in {}", entry.relative));
            return Ok(());
        };
        let cursor = {
            let document = self
                .state
                .documents
                .get(&id)
                .ok_or(Error::UnknownDocument(id))?;
            document.char_of_line(line)
        };
        let pane_id = self.state.focused_pane_id().ok_or(Error::NoFocusedPane)?;
        let pane = self
            .state
            .panes
            .get_mut(&pane_id)
            .ok_or(Error::UnknownPane(pane_id))?;
        pane.cursor = cursor;
        pane.anchor = None;
        pane.target_col = None;
        self.ensure_cursor_visible();
        self.notify(format!("conflict at {}:{}", entry.relative, line + 1));
        Ok(())
    }
}
