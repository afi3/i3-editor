//! Pane rendering: tab strip, gutter, text, cursor.

use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::editor::document::Document;
use crate::editor::text as textutil;
use crate::state::{AppState, Focus, Pane};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextGeometry {
    /// Rows and columns available for text, excluding borders and the tab strip.
    pub area: Rect,
    pub gutter: u16,
    pub text_width: u16,
}

/// Text region of a pane. Rendering and mouse hit-testing both use this so a
/// click lands on the cell it visually points at.
pub fn text_geometry(
    state: &AppState,
    pane: &Pane,
    document: &Document,
    area: Rect,
) -> Option<TextGeometry> {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let show_tabs =
        state.config.appearance.show_document_tabs && inner.height >= 3 && !pane.tabs.is_empty();
    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(u16::from(show_tabs)),
        inner.width,
        inner.height.saturating_sub(u16::from(show_tabs)),
    );
    if body.width == 0 || body.height == 0 {
        return None;
    }
    let digits = document.line_count().to_string().len() as u16;
    let show_numbers = state.config.editor.line_numbers && body.width > digits + 4;
    let gutter = if show_numbers { digits + 1 } else { 0 };
    let text_width = body.width.saturating_sub(gutter);
    if text_width == 0 {
        return None;
    }
    Some(TextGeometry {
        area: body,
        gutter,
        text_width,
    })
}

pub fn render(
    frame: &mut Frame,
    state: &AppState,
    pane: &Pane,
    document: Option<&Document>,
    area: Rect,
) {
    let focused = matches!(state.focus, Focus::Pane(id) if id == pane.id);
    let theme = state.theme;
    let border = Style::default().fg(if focused {
        theme.border_active
    } else {
        theme.border_inactive
    });
    let title = match document {
        Some(document) => format!(
            " {}{} ",
            document.display_name(),
            if document.dirty { " [+]" } else { "" }
        ),
        None => " [empty] ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(theme.base())
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let show_tabs =
        state.config.appearance.show_document_tabs && inner.height >= 3 && !pane.tabs.is_empty();
    if show_tabs {
        render_tabs(
            frame,
            state,
            pane,
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }

    let Some(document) = document else {
        return;
    };
    let Some(geometry) = text_geometry(state, pane, document, area) else {
        return;
    };
    let text_area = geometry.area;
    let gutter = geometry.gutter;
    let text_width = geometry.text_width;
    let show_numbers = gutter > 0;
    let line_count = document.line_count();
    let digits = line_count.to_string().len() as u16;
    let tab_width = state.config.editor.tab_size.max(1);
    let selection = pane.selection();
    let selected = theme.selection_style();
    let search = state
        .search
        .as_ref()
        .filter(|search| search.document == document.id);
    let match_starts = search.map(|search| search.matches.as_slice());
    let match_len = search
        .map(|search| search.query.chars().count())
        .unwrap_or(0);
    let matched = selected.add_modifier(Modifier::UNDERLINED);
    let number_style = Style::default().bg(theme.background).fg(theme.gutter);
    let cursor_line = document.line_of_char(pane.cursor);

    let buffer = frame.buffer_mut();
    let visible = line_count
        .saturating_sub(pane.scroll_top)
        .min(text_area.height as usize);
    let highlight = document.syntax_spans_range(pane.scroll_top, visible);
    for row in 0..text_area.height as usize {
        let line_index = pane.scroll_top + row;
        if line_index >= line_count {
            break;
        }
        let y = text_area.y.saturating_add(row as u16);
        let highlighted = focused && line_index == cursor_line;
        let row_style = if highlighted {
            theme.current_line()
        } else {
            theme.base()
        };
        buffer.set_style(Rect::new(text_area.x, y, text_area.width, 1), row_style);
        if show_numbers {
            let label = format!("{:>width$} ", line_index + 1, width = digits as usize);
            let style = if highlighted {
                Style::default()
                    .bg(theme.current_line)
                    .fg(theme.gutter_active)
            } else {
                number_style
            };
            buffer.set_stringn(text_area.x, y, &label, gutter as usize, style);
        }
        let text = document.line_text(line_index);
        let line_start = document.char_of_line(line_index);
        let spans = highlight.get(row).map(Vec::as_slice).unwrap_or(&[]);
        draw_text_line(
            buffer,
            text_area.x.saturating_add(gutter),
            y,
            text_width,
            &text,
            line_start,
            pane.scroll_left,
            tab_width,
            &selection,
            row_style,
            selected,
            spans,
            &theme,
            match_starts,
            match_len,
            matched,
        );
    }

    if focused {
        let line = document.line_of_char(pane.cursor);
        let line_start = document.char_of_line(line);
        let text = document.line_text(line);
        let column =
            textutil::cells_prefix(&text, pane.cursor.saturating_sub(line_start), tab_width);
        let visible_row = line.saturating_sub(pane.scroll_top);
        let x = text_area
            .x
            .saturating_add(gutter)
            .saturating_add(column.saturating_sub(pane.scroll_left));
        let y = text_area.y.saturating_add(visible_row as u16);
        if visible_row < text_area.height as usize && x < text_area.x + text_area.width {
            frame.set_cursor_position((x, y));
        }
    }
}

fn render_tabs(frame: &mut Frame, state: &AppState, pane: &Pane, area: Rect) {
    let theme = state.theme;
    frame.buffer_mut().set_style(area, theme.base());
    let mut spans = Vec::new();
    for id in &pane.tabs {
        let Some(document) = state.documents.get(id) else {
            continue;
        };
        let active = pane.active == Some(*id);
        let style = if active {
            theme.tab_active_style()
        } else {
            theme.tab_inactive_style()
        };
        spans.push(Span::styled(
            format!(
                " {}{} ",
                document.display_name(),
                if document.dirty { "*" } else { "" }
            ),
            style,
        ));
        spans.push(Span::styled(
            "|",
            Style::default().bg(theme.background).fg(theme.gutter),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[allow(clippy::too_many_arguments)]
fn draw_text_line(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    line_start_char: usize,
    skip: u16,
    tab_width: usize,
    selection: &Option<Range<usize>>,
    base: Style,
    selected: Style,
    spans: &[crate::syntax::Span],
    theme: &crate::theme::Theme,
    match_starts: Option<&[usize]>,
    match_len: usize,
    matched: Style,
) {
    if width == 0 {
        return;
    }
    let limit = skip.saturating_add(width);
    let tab = tab_width.max(1) as u16;
    let mut cell = 0u16;
    let mut cursor_x = x;
    let mut run = String::new();
    let mut run_style = base;
    let mut span_index = 0usize;
    let mut match_index = match_starts.map_or(0, |starts| {
        starts.partition_point(|start| start.saturating_add(match_len) <= line_start_char)
    });

    for (offset, ch) in text.chars().enumerate() {
        while span_index < spans.len() && spans[span_index].end <= offset {
            span_index += 1;
        }
        let advance = if ch == '\t' {
            tab - (cell % tab)
        } else {
            UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4])) as u16
        };
        let next = cell.saturating_add(advance);
        let absolute = line_start_char + offset;
        let in_match = match match_starts {
            Some(starts) => {
                while match_index < starts.len()
                    && starts[match_index].saturating_add(match_len) <= absolute
                {
                    match_index += 1;
                }
                starts
                    .get(match_index)
                    .is_some_and(|start| *start <= absolute)
            }
            None => false,
        };
        let selected_char = matches!(
            selection,
            Some(range) if absolute >= range.start && absolute < range.end
        );
        let style = if selected_char {
            selected
        } else if in_match {
            matched
        } else if let Some(span) = spans.get(span_index).filter(|span| span.start <= offset) {
            base.fg(theme.token_color(span.kind))
        } else {
            base
        };

        if next > skip && cell < limit {
            if style != run_style {
                flush(buffer, &mut cursor_x, &mut run, run_style, y);
                run_style = style;
            }
            if ch == '\t' {
                let start = cell.max(skip);
                let end = next.min(limit);
                for _ in start..end {
                    run.push(' ');
                }
            } else if cell >= skip {
                run.push(ch);
            }
        }

        cell = next;
        if cell >= limit {
            break;
        }
    }
    flush(buffer, &mut cursor_x, &mut run, run_style, y);
}

fn flush(buffer: &mut Buffer, x: &mut u16, run: &mut String, style: Style, y: u16) {
    if run.is_empty() {
        return;
    }
    let width = UnicodeWidthStr::width(run.as_str()) as u16;
    buffer.set_stringn(*x, y, run.as_str(), width as usize, style);
    *x = x.saturating_add(width);
    run.clear();
}
