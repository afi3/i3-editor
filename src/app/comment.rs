//! Toggle line or block comments for the selected lines.

use crate::error::Result;
use crate::state::{AppState, Focus};

use super::App;

impl App {
    pub(crate) fn toggle_comment(&mut self) -> Result<()> {
        let Focus::Pane(pane_id) = self.state.focus else {
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
            let Some(document_id) = pane.active else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&document_id) else {
                return Ok(());
            };
            let language = crate::syntax::Language::from_path(document.path.as_deref());
            if let Some(prefix) = language.line_comment() {
                toggle_line_comments(document, pane, prefix);
            } else if let Some((open, close)) = language.block_comment() {
                toggle_block_comment(document, pane, open, close);
            } else {
                note = Some(format!("{language:?} has no comment syntax"));
            }
        }
        if let Some(note) = note {
            self.notify(note);
        }
        self.ensure_cursor_visible();
        Ok(())
    }
}

fn toggle_line_comments(
    document: &mut crate::editor::document::Document,
    pane: &mut crate::state::Pane,
    prefix: &str,
) {
    let (start_line, end_line) = selected_lines(document.rope(), pane.cursor, pane.anchor);
    let mut lines: Vec<String> = (start_line..=end_line)
        .map(|line| document.line_text(line))
        .collect();
    let with_content: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let commented = !with_content.is_empty()
        && with_content.iter().all(|line| {
            line.trim_start()
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        });

    let mut deltas = Vec::with_capacity(lines.len());
    for line in &mut lines {
        let indent = line.len() - line.trim_start().len();
        if commented {
            let rest = &line[indent..];
            if let Some(after) = rest.strip_prefix(prefix) {
                let space = usize::from(after.starts_with(' '));
                let removed = prefix.len() + space;
                line.replace_range(indent..indent + removed, "");
                deltas.push(-(removed as isize));
            } else {
                deltas.push(0);
            }
        } else {
            let has_text = !line[indent..].is_empty();
            line.insert_str(
                indent,
                &format!("{prefix}{}", if has_text { " " } else { "" }),
            );
            deltas.push((prefix.len() + usize::from(has_text)) as isize);
        }
    }

    let block_start = document.char_of_line(start_line);
    let block_end = if end_line + 1 < document.line_count() {
        document.char_of_line(end_line + 1)
    } else {
        document.len_chars()
    };
    // The guard comes first: `char(0)` panics on an empty rope.
    let trailing = block_end > block_start && document.rope().char(block_end - 1) == '\n';
    let eol = document.line_ending();
    let mut text = lines.join(eol);
    if trailing {
        text.push_str(eol);
    }
    let cursor = remap_position(pane.cursor, document, start_line, &deltas);
    let anchor = pane
        .anchor
        .map(|anchor| remap_position(anchor, document, start_line, &deltas));
    let scroll = (pane.scroll_top, pane.scroll_left);
    document.replace(block_start..block_end, &text, pane.cursor, cursor, scroll);
    pane.cursor = cursor;
    pane.anchor = anchor;
    pane.target_col = None;
}

/// Wrap or unwrap the selection (or trimmed current line) with block delimiters.
fn toggle_block_comment(
    document: &mut crate::editor::document::Document,
    pane: &mut crate::state::Pane,
    open: &str,
    close: &str,
) {
    let range = match pane.selection() {
        Some(selection) => selection,
        None => {
            let line = document.line_of_char(pane.cursor);
            let text = document.line_text(line);
            // Char counts, not byte lengths: non-ASCII text would shift the
            // range and could run past the end of the buffer.
            let start = text.chars().count() - text.trim_start().chars().count();
            let end = text.trim_end().chars().count();
            let line_start = document.char_of_line(line);
            (line_start + start)..(line_start + end.max(start))
        }
    };
    let selected = document.rope().slice(range.clone()).to_string();
    let text = if let Some(inner) = selected
        .strip_prefix(open)
        .and_then(|rest| rest.strip_suffix(close))
    {
        inner.trim().to_string()
    } else {
        format!("{open} {} {close}", selected.trim())
    };
    let cursor = range.start + text.chars().count();
    let scroll = (pane.scroll_top, pane.scroll_left);
    document.replace(range, &text, pane.cursor, cursor, scroll);
    pane.cursor = cursor;
    pane.anchor = None;
    pane.target_col = None;
}

fn remap_position(
    position: usize,
    document: &crate::editor::document::Document,
    start_line: usize,
    deltas: &[isize],
) -> usize {
    let line = document.line_of_char(position);
    let line_start = document.char_of_line(line);
    let column = position.saturating_sub(line_start);
    let delta = line
        .checked_sub(start_line)
        .and_then(|index| deltas.get(index))
        .copied()
        .unwrap_or(0);
    line_start + (column as isize + delta).max(0) as usize
}

use crate::app::lines::selected_lines;
