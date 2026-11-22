//! Cursor motion and viewport clamping.

use ropey::Rope;

use crate::command::Move;
use crate::editor::text as textutil;
use crate::error::Result;
use crate::state::{AppState, Focus, PaneId};

use super::App;

/// Horizontal and document-edge movements. Returns false for vertical moves so
/// the caller can handle wrapping.
fn horizontal_move(
    pane: &mut crate::state::Pane,
    rope: &Rope,
    line: usize,
    movement: &Move,
) -> bool {
    let cursor = pane.cursor;
    match movement {
        Move::Left => pane.cursor = textutil::prev_grapheme(rope, cursor),
        Move::Right => pane.cursor = textutil::next_grapheme(rope, cursor),
        Move::WordLeft => pane.cursor = textutil::prev_word(rope, cursor),
        Move::WordRight => pane.cursor = textutil::next_word(rope, cursor),
        Move::LineStart => pane.cursor = rope.line_to_char(line),
        Move::LineEnd => pane.cursor = line_content_end(rope, line),
        Move::DocStart => pane.cursor = 0,
        Move::DocEnd => pane.cursor = rope.len_chars(),
        _ => return false,
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn vertical_move(
    pane: &mut crate::state::Pane,
    document: &crate::editor::document::Document,
    rope: &Rope,
    line: usize,
    target_column: usize,
    step: isize,
    wrap_width: Option<u16>,
    tab_width: usize,
) {
    if let Some(width) = wrap_width {
        if let Some((position, desired)) = wrapped_vertical(
            rope,
            document,
            width,
            tab_width,
            pane.cursor,
            step,
            pane.target_col,
        ) {
            pane.cursor = position;
            pane.target_col = desired;
        }
    } else {
        let new_line =
            (line as isize + step).clamp(0, rope.len_lines().saturating_sub(1) as isize) as usize;
        let content_end = line_content_end(rope, new_line);
        let line_start = rope.line_to_char(new_line);
        pane.cursor = line_start + target_column.min(content_end - line_start);
        pane.target_col = Some(target_column);
    }
}

fn wrapped_vertical(
    rope: &Rope,
    document: &crate::editor::document::Document,
    width: u16,
    tab_width: usize,
    cursor: usize,
    step: isize,
    target_col: Option<usize>,
) -> Option<(usize, Option<usize>)> {
    let mut line = rope.char_to_line(cursor);
    let mut ranges = textutil::wrap_ranges(&document.line_text(line), width, tab_width);
    let column = cursor.saturating_sub(document.char_of_line(line));
    let mut index = ranges
        .iter()
        .rposition(|(start, _)| column >= *start)
        .unwrap_or(0);
    let desired = target_col.unwrap_or(column.saturating_sub(ranges[index].0));

    for _ in 0..step.unsigned_abs() {
        if step > 0 {
            if index + 1 < ranges.len() {
                index += 1;
            } else {
                let Some(next) = next_visible_line(document, line) else {
                    break;
                };
                line = next;
                ranges = textutil::wrap_ranges(&document.line_text(line), width, tab_width);
                index = 0;
            }
        } else if index > 0 {
            index -= 1;
        } else {
            let Some(previous) = previous_visible_line(document, line) else {
                break;
            };
            line = previous;
            ranges = textutil::wrap_ranges(&document.line_text(line), width, tab_width);
            index = ranges.len().saturating_sub(1);
        }
    }

    let (start, end) = ranges[index];
    let column = desired.min(end - start);
    Some((document.char_of_line(line) + start + column, Some(desired)))
}

fn next_visible_line(document: &crate::editor::document::Document, line: usize) -> Option<usize> {
    ((line + 1)..document.line_count()).find(|candidate| !document.is_hidden(*candidate))
}

fn previous_visible_line(
    document: &crate::editor::document::Document,
    line: usize,
) -> Option<usize> {
    (0..line)
        .rev()
        .find(|candidate| !document.is_hidden(*candidate))
}

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
            Focus::Explorer | Focus::Shell => return Ok(()),
        };
        let page = self.page_size(pane_id);
        let wrap_width = self.wrap_width(pane_id);
        let tab_width = self.state.config.editor.tab_size.max(1);
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

            if !horizontal_move(pane, rope, line, &movement) {
                let step: isize = match movement {
                    Move::Up => -1,
                    Move::Down => 1,
                    Move::PageUp => -page,
                    _ => page,
                };
                vertical_move(
                    pane,
                    document,
                    rope,
                    line,
                    target_column,
                    step,
                    wrap_width,
                    tab_width,
                );
            }
            if !vertical {
                pane.target_col = None;
            }
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    pub(crate) fn add_cursor(&mut self, direction: isize) -> Result<()> {
        let message = {
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
            let line = document.line_of_char(pane.cursor);
            let target = line as isize + direction;
            if target < 0 || target as usize >= document.line_count() {
                "no line to add a cursor on".to_string()
            } else {
                let target = target as usize;
                let column = pane.cursor.saturating_sub(document.char_of_line(line));
                let text = document.line_text(target);
                let column = column.min(text.chars().count());
                let position = document.char_of_line(target) + column;
                pane.add_cursor(position);
                format!("{} cursor(s)", pane.extra.len() + 1)
            }
        };
        self.notify(message);
        Ok(())
    }

    pub(crate) fn clear_cursors(&mut self) {
        let Some(pane) = self.state.focused_pane_mut() else {
            return;
        };
        pane.clear_extra();
        pane.anchor = None;
        self.notify("secondary cursors cleared");
    }

    pub(crate) fn toggle_word_wrap(&mut self) {
        self.state.config.editor.word_wrap = !self.state.config.editor.word_wrap;
        let state = if self.state.config.editor.word_wrap {
            "on"
        } else {
            "off"
        };
        self.notify(format!("word wrap {state}"));
        self.ensure_cursor_visible();
    }

    pub(crate) fn duplicate_line(&mut self) -> Result<()> {
        let pane_id = match self.state.focus {
            Focus::Pane(id) => id,
            Focus::Explorer | Focus::Shell => return Ok(()),
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
            let eol = document.line_ending();
            let insert_at = line_content_end(&document.rope, line);
            let scroll = (pane.scroll_top, pane.scroll_left);
            document.insert(insert_at, &format!("{eol}{text}"), pane.cursor, scroll);
            document.history.break_run();
            pane.cursor = insert_at + eol.len() + text.chars().count();
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

    fn wrap_width(&self, pane_id: PaneId) -> Option<u16> {
        if !self.state.config.editor.word_wrap {
            return None;
        }
        let pane = self.state.panes.get(&pane_id)?;
        let rect = self.state.ui.pane_rects.get(&pane_id).copied()?;
        let document = self.state.documents.get(&pane.active?)?;
        crate::ui::pane::text_geometry(&self.state, pane, document, rect)
            .map(|geometry| geometry.text_width)
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
        let wrap = self.state.config.editor.word_wrap;

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
        if wrap {
            let digits = document.line_count().to_string().len() as u16;
            let gutter = if show_numbers { digits + 2 } else { 0 };
            let width = rect.width.saturating_sub(2 + gutter).max(1);
            let column = pane.cursor.saturating_sub(document.char_of_line(line));
            let visual =
                textutil::visual_rows(document, pane.scroll_top, width, tab_width, rows + 1);
            let index = visual
                .iter()
                .position(|row| row.line == line && column >= row.start && column <= row.end);
            if index.is_none_or(|index| index >= rows) {
                pane.scroll_top = line;
            }
            pane.scroll_left = 0;
            return;
        }
        if line < pane.scroll_top {
            pane.scroll_top = line;
        } else if line >= pane.scroll_top + rows {
            pane.scroll_top = line + 1 - rows;
        }

        let digits = document.line_count().to_string().len() as u16;
        let gutter = if show_numbers { digits + 2 } else { 0 };
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
