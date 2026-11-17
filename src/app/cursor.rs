//! Cursor motion and viewport clamping.

use ropey::Rope;

use crate::command::Move;
use crate::editor::text as textutil;
use crate::error::Result;
use crate::state::{AppState, Focus, PaneId};

use super::App;

fn line_content_end(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_char(line);
    let mut end = start + rope.line(line).len_chars();
    while end > start && matches!(rope.char(end - 1), '\n' | '\r') {
        end -= 1;
    }
    end
}

impl App {
    pub(crate) fn move_cursor(&mut self, movement: Move, extend: bool) -> Result<()> {
        let pane_id = match self.state.focus {
            Focus::Pane(id) => id,
            Focus::Explorer => return Ok(()),
        };
        let page = self.page_size(pane_id);
        {
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
            let rope = &document.rope;
            let cursor = pane.cursor;
            let line_count = rope.len_lines();

            if extend && pane.anchor.is_none() {
                pane.anchor = Some(cursor);
            }
            if !extend {
                pane.anchor = None;
            }

            let vertical = matches!(
                movement,
                Move::Up | Move::Down | Move::PageUp | Move::PageDown
            );
            let line = rope.char_to_line(cursor);
            let current_column = cursor.saturating_sub(rope.line_to_char(line));
            let target_column = if vertical {
                pane.target_col.unwrap_or(current_column)
            } else {
                current_column
            };

            match movement {
                Move::Left => pane.cursor = textutil::prev_grapheme(rope, cursor),
                Move::Right => pane.cursor = textutil::next_grapheme(rope, cursor),
                Move::WordLeft => pane.cursor = textutil::prev_word(rope, cursor),
                Move::WordRight => pane.cursor = textutil::next_word(rope, cursor),
                Move::LineStart => pane.cursor = rope.line_to_char(line),
                Move::LineEnd => pane.cursor = line_content_end(rope, line),
                Move::DocStart => pane.cursor = 0,
                Move::DocEnd => pane.cursor = rope.len_chars(),
                Move::Up | Move::Down | Move::PageUp | Move::PageDown => {
                    let step: isize = match movement {
                        Move::Up => -1,
                        Move::Down => 1,
                        Move::PageUp => -page,
                        _ => page,
                    };
                    let new_line = (line as isize + step)
                        .clamp(0, line_count.saturating_sub(1) as isize)
                        as usize;
                    let content_end = line_content_end(rope, new_line);
                    let line_start = rope.line_to_char(new_line);
                    pane.cursor = line_start + target_column.min(content_end - line_start);
                    pane.target_col = Some(target_column);
                }
            }
            if !vertical {
                pane.target_col = None;
            }
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    pub(crate) fn duplicate_line(&mut self) -> Result<()> {
        let pane_id = match self.state.focus {
            Focus::Pane(id) => id,
            Focus::Explorer => return Ok(()),
        };
        {
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
            let line = document.line_of_char(pane.cursor);
            let text = document.line_text(line);
            let insert_at = line_content_end(&document.rope, line);
            let scroll = (pane.scroll_top, pane.scroll_left);
            document.insert(insert_at, &format!("\n{text}"), pane.cursor, scroll);
            document.history.break_run();
            pane.cursor = insert_at + 1 + text.chars().count();
            pane.anchor = None;
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    fn page_size(&self, pane_id: PaneId) -> isize {
        let tabs = u16::from(self.state.config.appearance.show_document_tabs);
        self.state
            .ui
            .pane_rects
            .get(&pane_id)
            .map(|rect| rect.height.saturating_sub(2 + tabs).max(1) as isize)
            .unwrap_or(10)
    }

    pub(crate) fn ensure_cursor_visible(&mut self) {
        let Some(pane_id) = self.state.focused_pane_id() else {
            return;
        };
        let Some(rect) = self.state.ui.pane_rects.get(&pane_id).copied() else {
            return;
        };
        let tab_width = self.state.config.editor.tab_size.max(1);
        let show_tabs = self.state.config.appearance.show_document_tabs;
        let show_numbers = self.state.config.editor.line_numbers;

        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let Some(pane) = panes.get_mut(&pane_id) else {
            return;
        };
        let Some(id) = pane.active else {
            return;
        };
        let Some(document) = documents.get(&id) else {
            return;
        };

        let rows = rect.height.saturating_sub(2 + u16::from(show_tabs)).max(1) as usize;
        let line = document.line_of_char(pane.cursor);
        if line < pane.scroll_top {
            pane.scroll_top = line;
        } else if line >= pane.scroll_top + rows {
            pane.scroll_top = line + 1 - rows;
        }

        let digits = document.line_count().to_string().len() as u16;
        let gutter = if show_numbers { digits + 1 } else { 0 };
        let width = rect.width.saturating_sub(2 + gutter).max(1);
        let line_start = document.char_of_line(line);
        let text = document.line_text(line);
        let column =
            textutil::cells_prefix(&text, pane.cursor.saturating_sub(line_start), tab_width);
        if column < pane.scroll_left {
            pane.scroll_left = column;
        } else if column >= pane.scroll_left + width {
            pane.scroll_left = column + 1 - width;
        }
    }
}
