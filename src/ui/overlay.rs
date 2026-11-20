//! Explorer and prompt overlays.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::editor::text as textutil;
use crate::input::{PromptKind, PromptState};
use crate::state::{AppState, Focus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExplorerGeometry {
    pub area: Rect,
    /// First visible row index.
    pub scroll: usize,
    pub rows: usize,
}

/// Inner explorer area and scroll offset, shared by rendering and mouse input.
pub fn explorer_geometry(state: &AppState) -> Option<ExplorerGeometry> {
    let outer = state.ui.explorer;
    if outer.width == 0 || outer.height == 0 {
        return None;
    }
    let inner = Block::default().borders(Borders::ALL).inner(outer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let rows = inner.height as usize;
    let selected = state.explorer.selected;
    let scroll = if selected >= rows {
        selected + 1 - rows
    } else {
        0
    };
    Some(ExplorerGeometry {
        area: inner,
        scroll,
        rows,
    })
}

pub fn render_explorer(frame: &mut Frame, state: &AppState) {
    let area = state.ui.explorer;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = state.focus == Focus::Explorer;
    let theme = state.theme;
    let border = Style::default().fg(if focused {
        theme.border_active
    } else {
        theme.border_inactive
    });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(theme.base())
        .title(" Explorer ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(geometry) = explorer_geometry(state) else {
        return;
    };
    let inner = geometry.area;
    let height = geometry.rows;
    let selected = state.explorer.selected;
    let scroll = geometry.scroll;

    let mut lines = Vec::new();
    for (index, row) in state
        .explorer
        .rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
    {
        let marker = if row.is_dir {
            if row.expanded { "v " } else { "> " }
        } else {
            "  "
        };
        let indent = "  ".repeat(row.depth);
        let text = format!("{indent}{marker}{}", row.name);
        let style = if index == selected {
            theme.selection_style()
        } else if row.is_dir {
            Style::default().bg(theme.background).fg(theme.explorer_dir)
        } else {
            theme.base()
        };
        lines.push(Line::from(Span::styled(text, style)));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Rows of the match list shown under the Find field: `(selected offset, rows)`.
/// A row is `(line number, line text)`.
fn find_rows(state: &AppState, prompt: &PromptState) -> Option<(usize, Vec<(usize, String)>)> {
    const MAX_ROWS: usize = 8;
    if prompt.kind != PromptKind::Find {
        return None;
    }
    let search = state.search.as_ref()?;
    let document = state.documents.get(&search.document)?;
    if Some(search.document) != state.active_document_id() || prompt.input() != search.query {
        return None;
    }
    let total = search.matches.len();
    if total == 0 {
        return None;
    }
    let selected = search.index.min(total - 1);
    let start = selected.saturating_sub(MAX_ROWS - 1);
    let end = (start + MAX_ROWS).min(total);
    let rows = search.matches[start..end]
        .iter()
        .map(|offset| {
            let line = document.line_of_char(*offset);
            (line + 1, document.line_text(line))
        })
        .collect();
    Some((selected - start, rows))
}

pub fn render_prompt(frame: &mut Frame, state: &AppState, prompt: &PromptState) {
    let screen = frame.area();
    let width = ((u32::from(screen.width) * 70 / 100) as u16).clamp(30, 100);
    let width = width.min(screen.width.max(1));
    let candidates = if prompt.kind.lists_candidates() {
        prompt.filtered.len().min(8) as u16
    } else {
        0
    };
    let matches = find_rows(state, prompt);
    let match_rows = matches.as_ref().map_or(0, |(_, rows)| rows.len() as u16);
    let target = if prompt.kind == PromptKind::Overwrite {
        prompt
            .target
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
    } else {
        None
    };
    let target_rows = u16::from(target.is_some());
    let fields = prompt.fields.len() as u16;
    let height = (fields + candidates + match_rows + target_rows + 2).min(screen.height.max(1));
    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = screen.y + screen.height.saturating_sub(height) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, area);
    let theme = state.theme;
    frame.buffer_mut().set_style(area, theme.base());
    let title = match (prompt.kind, state.search.as_ref()) {
        (PromptKind::Find, Some(search))
            if Some(search.document) == state.active_document_id()
                && search.query == prompt.input()
                && !search.matches.is_empty() =>
        {
            format!(" Find {}/{} ", search.index + 1, search.matches.len())
        }
        _ => format!(" {} ", prompt.kind.title()),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.prompt_border))
        .style(theme.base())
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let tab_width = state.config.editor.tab_size.max(1);
    let mut cursor_position = None;
    for (index, value) in prompt.fields.iter().enumerate() {
        let row = inner.y.saturating_add(index as u16);
        if row >= inner.y + inner.height {
            break;
        }
        let label = field_label(prompt.kind, index);
        let label_width = textutil::cells(label, tab_width, 0);
        let input_width = inner.width.saturating_sub(label_width);
        let buffer = frame.buffer_mut();
        buffer.set_stringn(
            inner.x,
            row,
            label,
            label_width as usize,
            Style::default().bg(theme.background).fg(theme.gutter),
        );
        let shown = textutil::clip(value, 0, input_width, tab_width);
        buffer.set_stringn(
            inner.x + label_width,
            row,
            &shown,
            input_width as usize,
            theme.base(),
        );

        if index == prompt.active {
            let cursor = textutil::cells_prefix(value, prompt.cursor, tab_width);
            let cursor_x = inner
                .x
                .saturating_add(label_width)
                .saturating_add(cursor.min(input_width.saturating_sub(1)));
            cursor_position = Some((cursor_x, row));
        }
    }

    if candidates > 0 {
        let list_top = inner.y.saturating_add(fields);
        let visible = prompt.visible();
        let start = if prompt.selected >= candidates as usize {
            prompt.selected + 1 - candidates as usize
        } else {
            0
        };
        let buffer = frame.buffer_mut();
        for (offset, candidate) in visible
            .iter()
            .skip(start)
            .take(candidates as usize)
            .enumerate()
        {
            let row = list_top.saturating_add(offset as u16);
            if row >= inner.y + inner.height {
                break;
            }
            let style = if start + offset == prompt.selected {
                theme.selection_style()
            } else {
                theme.base()
            };
            buffer.set_stringn(inner.x, row, candidate, inner.width as usize, style);
            if let (PromptKind::CommandPalette, Some(command)) =
                (prompt.kind, crate::command::Command::from_name(candidate))
            {
                let offset = candidate.chars().count() as u16 + 2;
                if offset < inner.width {
                    buffer.set_stringn(
                        inner.x + offset,
                        row,
                        command.description(),
                        (inner.width - offset) as usize,
                        Style::default().add_modifier(Modifier::DIM),
                    );
                }
            }
        }
    }

    if let Some(path) = target {
        let shown = tail(&path, inner.width);
        frame.buffer_mut().set_stringn(
            inner.x,
            inner.y,
            &shown,
            inner.width as usize,
            Style::default().bg(theme.background).fg(theme.gutter),
        );
    }

    if let Some((selected, rows)) = matches {
        let top = inner.y.saturating_add(fields);
        let buffer = frame.buffer_mut();
        for (offset, (line, text)) in rows.iter().enumerate() {
            let row = top.saturating_add(offset as u16);
            if row >= inner.y.saturating_add(inner.height) {
                break;
            }
            let style = if offset == selected {
                theme.selection_style()
            } else {
                theme.base()
            };
            let label = format!("{line:>5}  {}", text.trim_end());
            buffer.set_stringn(inner.x, row, &label, inner.width as usize, style);
        }
    }

    if let Some((x, y)) = cursor_position {
        frame.set_cursor_position((x, y));
    }
}

/// Clip a path from the left so its file name stays visible.
fn tail(text: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let mut out = String::from("…");
    out.extend(chars[chars.len() - (width - 1)..].iter());
    out
}

fn field_label(kind: PromptKind, index: usize) -> &'static str {
    match (kind, index) {
        (PromptKind::Replace, 0) => "Find: ",
        (PromptKind::Replace, 1) => "With: ",
        (PromptKind::Find, _) => "Find: ",
        (PromptKind::GoToLine, _) => "Line: ",
        (PromptKind::CommandPalette, _) => "> ",
        _ => "",
    }
}
