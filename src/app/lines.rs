//! Line-level structural edits: move a block, delete lines.

use ropey::Rope;

use crate::error::Result;
use crate::state::{AppState, DocumentId, Focus, PaneId};

use super::App;

impl App {
    /// Move the current line or selection block by `direction` (-1 up, 1 down).
    pub(crate) fn move_lines(&mut self, direction: isize) -> Result<()> {
        let Some((pane_id, document_id)) = self.target_pane_document() else {
            return Ok(());
        };
        let mut note = None;
        {
            let AppState {
                documents, panes, ..
            } = &mut self.state;
            let Some(pane) = panes.get_mut(&pane_id) else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&document_id) else {
                return Ok(());
            };
            let cursor = pane.cursor;
            let anchor = pane.anchor;
            let eol = document.line_ending();
            let (start_line, end_line) = selected_lines(document.rope(), cursor, anchor);
            let line_count = document.line_count();
            let (range, text, shift) = if direction < 0 {
                if start_line == 0 {
                    note = Some("already at the first line");
                    (0..0, String::new(), 0isize)
                } else {
                    let line_start = document.char_of_line(start_line);
                    let block_end = if end_line + 1 < line_count {
                        document.char_of_line(end_line + 1)
                    } else {
                        document.len_chars()
                    };
                    let previous_start = document.char_of_line(start_line - 1);
                    let block = document.rope().slice(line_start..block_end).to_string();
                    let previous = document
                        .rope()
                        .slice(previous_start..line_start)
                        .to_string();
                    let text = if block.ends_with('\n') {
                        format!("{block}{previous}")
                    } else {
                        format!("{block}{eol}{}", previous.trim_end_matches(['\r', '\n']))
                    };
                    (
                        previous_start..block_end,
                        text,
                        previous_start as isize - line_start as isize,
                    )
                }
            } else if end_line + 1 >= line_count {
                note = Some("already at the last line");
                (0..0, String::new(), 0isize)
            } else {
                let line_start = document.char_of_line(start_line);
                let next_start = document.char_of_line(end_line + 1);
                let next_end = if end_line + 2 < line_count {
                    document.char_of_line(end_line + 2)
                } else {
                    document.len_chars()
                };
                let block = document.rope().slice(line_start..next_start).to_string();
                let next = document.rope().slice(next_start..next_end).to_string();
                let shift = if next.ends_with('\n') {
                    next.chars().count() as isize
                } else {
                    next.chars().count() as isize + eol.chars().count() as isize
                };
                let text = if next.ends_with('\n') {
                    format!("{next}{block}")
                } else {
                    format!("{next}{eol}{}", block.trim_end_matches(['\r', '\n']))
                };
                (line_start..next_end, text, shift)
            };

            if note.is_none() {
                let new_cursor = shifted(cursor, shift);
                let new_anchor = anchor.map(|anchor| shifted(anchor, shift));
                let scroll = (pane.scroll_top, pane.scroll_left);
                document.replace(range, &text, cursor, new_cursor, scroll);
                pane.cursor = new_cursor;
                pane.anchor = new_anchor;
                pane.target_col = None;
            }
        }
        if let Some(note) = note {
            self.notify(note);
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    pub(crate) fn delete_line(&mut self) -> Result<()> {
        let Some((pane_id, document_id)) = self.target_pane_document() else {
            return Ok(());
        };
        {
            let AppState {
                documents, panes, ..
            } = &mut self.state;
            let Some(pane) = panes.get_mut(&pane_id) else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&document_id) else {
                return Ok(());
            };
            let cursor = pane.cursor;
            let (start_line, end_line) = selected_lines(document.rope(), cursor, pane.anchor);
            let line_start = document.char_of_line(start_line);
            let line_count = document.line_count();
            let range = if end_line + 1 < line_count {
                line_start..document.char_of_line(end_line + 1)
            } else if line_start == document.len_chars() {
                let mut start = line_start.saturating_sub(1);
                // CRLF: the line feed alone would leave a stray carriage return.
                if start > 0 && document.rope().char(start - 1) == '\r' {
                    start -= 1;
                }
                start..document.len_chars()
            } else {
                line_start..document.len_chars()
            };
            let scroll = (pane.scroll_top, pane.scroll_left);
            document.remove(range.clone(), cursor, scroll);
            pane.cursor = range.start.min(document.len_chars());
            pane.anchor = None;
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    fn target_pane_document(&self) -> Option<(PaneId, DocumentId)> {
        let Focus::Pane(pane_id) = self.state.focus else {
            return None;
        };
        let pane = self.state.panes.get(&pane_id)?;
        Some((pane_id, pane.active?))
    }
}

/// Inclusive line range covered by the selection, excluding the line that only
/// contains the selection's end boundary.
pub(super) fn selected_lines(rope: &Rope, cursor: usize, anchor: Option<usize>) -> (usize, usize) {
    let (start, end) = match anchor {
        Some(anchor) if anchor < cursor => (anchor, cursor),
        Some(anchor) if anchor > cursor => (cursor, anchor),
        _ => (cursor, cursor),
    };
    let start_line = rope.char_to_line(start.min(rope.len_chars()));
    let mut end_line = rope.char_to_line(end.min(rope.len_chars()));
    if end > start && end_line > start_line && rope.line_to_char(end_line) == end {
        end_line -= 1;
    }
    (start_line, end_line.min(rope.len_lines().saturating_sub(1)))
}

fn shifted(position: usize, shift: isize) -> usize {
    (position as isize + shift).max(0) as usize
}
