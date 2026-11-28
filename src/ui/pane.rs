//! Pane rendering: tab strip, gutter, text, cursor.

use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::editor::document::Document;
use crate::editor::text as textutil;
use crate::state::{AppState, DocumentId, Focus, Pane};

pub const DOCUMENT_TAB_CLOSE: &str = "×";
pub const TAB_SCROLL_LEFT: &str = "‹";
pub const TAB_SCROLL_RIGHT: &str = "›";

/// One document tab in the strip. `start` and `width` are cell offsets relative
/// to the strip content, including the close button and the `|` separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentTab {
    pub id: DocumentId,
    pub label: String,
    pub label_width: u16,
    pub start: u16,
    pub width: u16,
}

impl DocumentTab {
    pub fn close_start(&self) -> u16 {
        self.start.saturating_add(self.label_width)
    }
}

/// The visible tab-strip region and its horizontal scroll state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabStripGeometry {
    pub content: Rect,
    pub overflow: bool,
    pub scroll: u16,
    pub max_scroll: u16,
}

pub fn document_tabs(state: &AppState, pane: &Pane, tab_width: usize) -> Vec<DocumentTab> {
    let mut start = 0u16;
    let mut tabs = Vec::with_capacity(pane.tabs.len());
    for id in &pane.tabs {
        let Some(document) = state.documents.get(id) else {
            continue;
        };
        let label = format!(
            " {}{} ",
            document.display_name(),
            if document.dirty { "*" } else { "" }
        );
        let label_width = textutil::cells(&label, tab_width, 0);
        let width = label_width.saturating_add(2);
        tabs.push(DocumentTab {
            id: *id,
            label,
            label_width,
            start,
            width,
        });
        start = start.saturating_add(width);
    }
    tabs
}

/// Reserve one cell on each side for scroll arrows only when the tabs overflow.
pub fn tab_strip_geometry(area: Rect, total: u16, requested: u16) -> TabStripGeometry {
    let overflow = area.width > 2 && total > area.width;
    if !overflow {
        return TabStripGeometry {
            content: area,
            overflow: false,
            scroll: 0,
            max_scroll: 0,
        };
    }
    let content = Rect::new(
        area.x.saturating_add(1),
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    let max_scroll = total.saturating_sub(content.width);
    TabStripGeometry {
        content,
        overflow: true,
        scroll: requested.min(max_scroll),
        max_scroll,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextGeometry {
    /// Rows and columns available for text, excluding borders and the tab strip.
    pub area: Rect,
    pub gutter: u16,
    pub text_width: u16,
    /// One column at the right edge is reserved for the vertical scrollbar.
    pub scrollbar: bool,
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
    let show_numbers = state.config.editor.line_numbers && body.width > digits + 5;
    let gutter = if show_numbers { digits + 2 } else { 0 };
    let scrollbar = document.line_count() > usize::from(body.height);
    let text_width = body
        .width
        .saturating_sub(gutter)
        .saturating_sub(u16::from(scrollbar));
    if text_width == 0 {
        return None;
    }
    Some(TextGeometry {
        area: body,
        gutter,
        text_width,
        scrollbar,
    })
}

/// Vertical scrollbar geometry for a document with more lines than the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    pub track: Rect,
    pub thumb_start: u16,
    pub thumb_len: u16,
    pub max_top: usize,
    pub rows: usize,
}

pub fn scrollbar_geometry(
    state: &AppState,
    pane: &Pane,
    document: &Document,
    area: Rect,
) -> Option<ScrollbarGeometry> {
    let geometry = text_geometry(state, pane, document, area)?;
    if !geometry.scrollbar {
        return None;
    }
    let rows = usize::from(geometry.area.height);
    let total = document.line_count();
    let max_top = total.saturating_sub(rows);
    if max_top == 0 || geometry.area.width == 0 {
        return None;
    }
    let thumb_len = (rows * rows / total).max(1).min(rows) as u16;
    let travel = rows.saturating_sub(usize::from(thumb_len));
    let top = pane.scroll_top.min(max_top);
    let thumb_start = if travel == 0 {
        0
    } else {
        (top * travel / max_top) as u16
    };
    Some(ScrollbarGeometry {
        track: Rect::new(
            geometry
                .area
                .x
                .saturating_add(geometry.area.width)
                .saturating_sub(1),
            geometry.area.y,
            1,
            geometry.area.height,
        ),
        thumb_start,
        thumb_len,
        max_top,
        rows,
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
    let tab_width = document.indent.tab_width.max(1);
    let selections = pane.selected_ranges();
    let selected = theme.selection_style();
    let search = state
        .search
        .as_ref()
        .filter(|search| search.document == document.id);
    let match_ranges = search.map(|search| search.matches.as_slice());
    let matched = selected.add_modifier(Modifier::UNDERLINED);
    let number_style = Style::default().bg(theme.background).fg(theme.gutter);
    let cursor_line = document.line_of_char(pane.cursor);
    let severities = line_severities(state, document);

    let buffer = frame.buffer_mut();
    let wrap = state.config.editor.word_wrap;
    let rows = visual_rows_for(
        state,
        document,
        pane,
        text_width,
        tab_width,
        text_area.height as usize,
    );
    // The row list is ordered by line, so a line's text, its start offset and
    // its syntax spans are fetched once and reused by its wrapped rows. Doing
    // this per row made every frame cost O(line length) per visible row.
    let mut current_line: Option<usize> = None;
    let mut line_text = String::new();
    let mut line_start = 0usize;
    let mut line_len = 0usize;
    let mut line_spans: Vec<crate::syntax::Span> = Vec::new();
    let mut span_cursor = 0usize;
    for (row, visual) in rows.iter().enumerate() {
        let line_index = visual.line;
        let first_row = visual.start == 0;
        let highlighted = focused && line_index == cursor_line;
        let y = text_area.y.saturating_add(row as u16);
        let row_style = if highlighted {
            theme.current_line()
        } else {
            theme.base()
        };
        buffer.set_style(Rect::new(text_area.x, y, text_area.width, 1), row_style);
        if show_numbers && first_row {
            let background = if highlighted {
                theme.current_line
            } else {
                theme.background
            };
            render_gutter_cell(
                buffer,
                state,
                document,
                line_index,
                text_area.x,
                y,
                usize::from(digits),
                highlighted,
                number_style,
                background,
                severities.get(&line_index).copied(),
            );
        }
        if current_line != Some(line_index) {
            line_start = document.char_of_line(line_index);
            line_len = document.line_len(line_index);
            line_text = document.line_text(line_index);
            line_spans = document
                .syntax_spans_range(line_index, 1)
                .first()
                .cloned()
                .unwrap_or_default();
            span_cursor = 0;
            current_line = Some(line_index);
        }
        let text: std::borrow::Cow<str> = if visual.start == 0 && visual.end == line_len {
            std::borrow::Cow::Borrowed(line_text.as_str())
        } else {
            let start = line_start + visual.start;
            std::borrow::Cow::Owned(
                document
                    .rope()
                    .slice(start..start + (visual.end - visual.start))
                    .to_string(),
            )
        };
        let row_start = line_start + visual.start;
        // Spans are ordered too: advance past the ones already scrolled out
        // instead of rescanning the whole line for every row.
        while span_cursor < line_spans.len() && line_spans[span_cursor].end <= visual.start {
            span_cursor += 1;
        }
        let spans: Vec<crate::syntax::Span> = if wrap {
            line_spans[span_cursor..]
                .iter()
                .take_while(|span| span.start < visual.end)
                .map(|span| crate::syntax::Span {
                    start: span.start.saturating_sub(visual.start),
                    end: span
                        .end
                        .saturating_sub(visual.start)
                        .min(visual.end - visual.start),
                    kind: span.kind,
                })
                .collect()
        } else {
            line_spans.clone()
        };
        draw_text_line(
            buffer,
            text_area.x.saturating_add(gutter),
            y,
            text_width,
            &text,
            row_start,
            if wrap { 0 } else { pane.scroll_left },
            tab_width,
            &selections,
            row_style,
            selected,
            &spans,
            &theme,
            match_ranges,
            matched,
        );
    }

    if focused && !wrap {
        highlight_brackets(frame, pane, document, text_area, gutter, tab_width);
    }

    draw_scrollbar(frame, state, pane, document, area, focused);

    if focused {
        if let Some((visible_row, column)) =
            cursor_visual_row(&rows, pane.cursor, document, wrap, tab_width)
            && let Some((x, y)) = caret_cell(
                text_area,
                gutter,
                column,
                visible_row,
                pane.scroll_left,
                wrap,
                geometry.scrollbar,
            )
        {
            frame.set_cursor_position((x, y));
        }
        draw_extra_carets(
            frame,
            pane,
            document,
            &rows,
            text_area,
            gutter,
            wrap,
            tab_width,
            geometry.scrollbar,
        );
    }
}

fn draw_scrollbar(
    frame: &mut Frame,
    state: &AppState,
    pane: &Pane,
    document: &Document,
    area: Rect,
    focused: bool,
) {
    let Some(scrollbar) = scrollbar_geometry(state, pane, document, area) else {
        return;
    };
    let theme = state.theme;
    let track = Style::default().bg(theme.background).fg(theme.gutter);
    let thumb = Style::default()
        .bg(theme.background)
        .fg(if focused {
            theme.border_active
        } else {
            theme.border_inactive
        })
        .add_modifier(Modifier::BOLD);
    let buffer = frame.buffer_mut();
    let x = scrollbar.track.x;
    for offset in 0..scrollbar.track.height {
        buffer[(x, scrollbar.track.y.saturating_add(offset))]
            .set_symbol("│")
            .set_style(track);
    }
    for offset in scrollbar.thumb_start..scrollbar.thumb_start + scrollbar.thumb_len {
        buffer[(x, scrollbar.track.y.saturating_add(offset))]
            .set_symbol("█")
            .set_style(thumb);
    }
}

fn caret_cell(
    text_area: Rect,
    gutter: u16,
    column: u16,
    row: usize,
    scroll_left: u16,
    wrap: bool,
    scrollbar: bool,
) -> Option<(u16, u16)> {
    if row >= text_area.height as usize {
        return None;
    }
    let column = if wrap {
        column
    } else {
        column.checked_sub(scroll_left)?
    };
    let x = text_area.x.saturating_add(gutter).saturating_add(column);
    let edge = text_area
        .x
        .saturating_add(text_area.width)
        .saturating_sub(u16::from(scrollbar));
    (x < edge).then_some((x, text_area.y.saturating_add(row as u16)))
}

#[allow(clippy::too_many_arguments)]
fn draw_extra_carets(
    frame: &mut Frame,
    pane: &Pane,
    document: &Document,
    rows: &[textutil::VisualRow],
    text_area: Rect,
    gutter: u16,
    wrap: bool,
    tab_width: usize,
    scrollbar: bool,
) {
    if pane.extra.is_empty() {
        return;
    }
    let caret = Style::default().add_modifier(Modifier::REVERSED);
    let positions: Vec<(u16, u16)> = pane
        .extra
        .iter()
        .filter_map(|extra| {
            let (row, column) = cursor_visual_row(rows, extra.cursor, document, wrap, tab_width)?;
            caret_cell(
                text_area,
                gutter,
                column,
                row,
                pane.scroll_left,
                wrap,
                scrollbar,
            )
        })
        .collect();
    let buffer = frame.buffer_mut();
    for (x, y) in positions {
        buffer[(x, y)].set_style(caret);
    }
}

/// Document lines that fit in `rows`, skipping closed folds.
fn visible_lines(
    document: &crate::editor::document::Document,
    start: usize,
    rows: usize,
) -> Vec<usize> {
    let mut lines = Vec::with_capacity(rows);
    for line in start..document.line_count() {
        if lines.len() >= rows {
            break;
        }
        if !document.is_hidden(line) {
            lines.push(line);
        }
    }
    lines
}

/// Lowest severity per line for one document, built once per draw instead of
/// scanning the whole diagnostic list for every rendered row.
fn line_severities(
    state: &AppState,
    document: &Document,
) -> std::collections::HashMap<usize, crate::lsp::Severity> {
    let mut severities = std::collections::HashMap::new();
    let Some(diagnostics) = state.lsp_diagnostics.get(&document.id) else {
        return severities;
    };
    for diagnostic in diagnostics {
        let entry = severities
            .entry(diagnostic.line)
            .or_insert(diagnostic.severity);
        if diagnostic.severity < *entry {
            *entry = diagnostic.severity;
        }
    }
    severities
}

#[allow(clippy::too_many_arguments)]
fn render_gutter_cell(
    buffer: &mut Buffer,
    state: &AppState,
    document: &Document,
    line_index: usize,
    x: u16,
    y: u16,
    digits: usize,
    highlighted: bool,
    number_style: Style,
    background: Color,
    severity: Option<crate::lsp::Severity>,
) {
    let digit_cells = u16::try_from(digits).unwrap_or(u16::MAX);
    let label = format!("{:>width$}", line_index + 1, width = digits);
    let style = if highlighted {
        Style::default()
            .bg(background)
            .fg(state.theme.gutter_active)
    } else {
        number_style
    };
    buffer.set_stringn(x, y, &label, digits, style);
    if let Some(change) = state
        .git_marks
        .get(&document.id)
        .and_then(|marks| marks.get(line_index).copied().flatten())
    {
        let (symbol, color) = match change {
            crate::git::Change::Added => ('+', Color::Green),
            crate::git::Change::Modified => ('~', Color::Yellow),
            crate::git::Change::Deleted => ('-', Color::Red),
        };
        buffer.set_stringn(
            x.saturating_add(digit_cells),
            y,
            symbol.to_string(),
            1,
            Style::default().bg(background).fg(color),
        );
    }
    let Some(severity) = severity else {
        return;
    };
    let color = match severity {
        crate::lsp::Severity::Error => Color::Red,
        crate::lsp::Severity::Warning => Color::Yellow,
        crate::lsp::Severity::Info => Color::Cyan,
    };
    buffer.set_stringn(
        x.saturating_add(digit_cells.saturating_add(1)),
        y,
        "•",
        1,
        Style::default().bg(background).fg(color),
    );
}

fn visual_rows_for(
    state: &AppState,
    document: &crate::editor::document::Document,
    pane: &Pane,
    width: u16,
    tab_width: usize,
    height: usize,
) -> Vec<textutil::VisualRow> {
    if state.config.editor.word_wrap {
        let keep = height.saturating_add(pane.scroll_row);
        return textutil::visual_rows(document, pane.scroll_top, width, tab_width, keep)
            .into_iter()
            .skip(pane.scroll_row)
            .collect();
    }
    visible_lines(document, pane.scroll_top, height)
        .into_iter()
        .map(|line| textutil::VisualRow {
            line,
            start: 0,
            end: document.line_len(line),
        })
        .collect()
}

fn cursor_visual_row(
    rows: &[textutil::VisualRow],
    cursor: usize,
    document: &crate::editor::document::Document,
    wrap: bool,
    tab_width: usize,
) -> Option<(usize, u16)> {
    let line = document.line_of_char(cursor);
    let column = cursor.saturating_sub(document.char_of_line(line));
    if wrap {
        let full = document.line_text(line);
        let (index, row) = rows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| row.line == line && column >= row.start)?;
        let prefix: String = full
            .chars()
            .skip(row.start)
            .take(column - row.start)
            .collect();
        return Some((index, textutil::cells(&prefix, tab_width, 0)));
    }
    let index = rows.iter().position(|row| row.line == line)?;
    let text = document.line_text(line);
    Some((index, textutil::cells_prefix(&text, column, tab_width)))
}

fn render_tabs(frame: &mut Frame, state: &AppState, pane: &Pane, area: Rect) {
    let theme = state.theme;
    frame.buffer_mut().set_style(area, theme.base());
    let tab_width = state.config.editor.tab_size.max(1);
    let tabs = document_tabs(state, pane, tab_width);
    let total = tabs
        .last()
        .map(|tab| tab.start.saturating_add(tab.width))
        .unwrap_or(0);
    let geometry = tab_strip_geometry(area, total, pane.tab_scroll);
    let mut spans = Vec::with_capacity(tabs.len() * 3);
    for tab in &tabs {
        let active = pane.active == Some(tab.id);
        let style = if active {
            theme.tab_active_style()
        } else {
            theme.tab_inactive_style()
        };
        spans.push(Span::styled(tab.label.clone(), style));
        spans.push(Span::styled(DOCUMENT_TAB_CLOSE, style));
        spans.push(Span::styled(
            "|",
            Style::default().bg(theme.background).fg(theme.gutter),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).scroll((0, geometry.scroll)),
        geometry.content,
    );
    if geometry.overflow {
        let left = if geometry.scroll > 0 {
            theme.tab_active_style()
        } else {
            theme.tab_inactive_style()
        };
        let right = if geometry.scroll < geometry.max_scroll {
            theme.tab_active_style()
        } else {
            theme.tab_inactive_style()
        };
        let buffer = frame.buffer_mut();
        let right_x = area.x.saturating_add(area.width).saturating_sub(1);
        buffer[(area.x, area.y)]
            .set_symbol(TAB_SCROLL_LEFT)
            .set_style(left);
        buffer[(right_x, area.y)]
            .set_symbol(TAB_SCROLL_RIGHT)
            .set_style(right);
    }
}

fn highlight_brackets(
    frame: &mut ratatui::Frame,
    pane: &Pane,
    document: &crate::editor::document::Document,
    text_area: Rect,
    gutter: u16,
    tab_width: usize,
) {
    let Some((first, second)) = textutil::matching_bracket(document.rope(), pane.cursor) else {
        return;
    };
    let buffer = frame.buffer_mut();
    for position in [first, second] {
        let line = document.line_of_char(position);
        if line < pane.scroll_top
            || line >= pane.scroll_top + usize::from(text_area.height)
            || document.is_hidden(line)
        {
            continue;
        }
        let line_start = document.char_of_line(line);
        let text = document.line_text(line);
        let column = textutil::cells_prefix(&text, position.saturating_sub(line_start), tab_width);
        if column < pane.scroll_left {
            continue;
        }
        let x = text_area
            .x
            .saturating_add(gutter)
            .saturating_add(column - pane.scroll_left);
        if x >= text_area.x.saturating_add(text_area.width) {
            continue;
        }
        let visible_row = (pane.scroll_top..line)
            .filter(|candidate| !document.is_hidden(*candidate))
            .count();
        let y = text_area.y.saturating_add(visible_row as u16);
        buffer[(x, y)].modifier |= Modifier::BOLD | Modifier::UNDERLINED;
    }
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
    selection: &[Range<usize>],
    base: Style,
    selected: Style,
    spans: &[crate::syntax::Span],
    theme: &crate::theme::Theme,
    match_ranges: Option<&[std::ops::Range<usize>]>,
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
    let mut match_index = match_ranges.map_or(0, |ranges| {
        ranges.partition_point(|range| range.end <= line_start_char)
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
        let in_match =
            match_ranges.is_some_and(|ranges| in_match(ranges, &mut match_index, absolute));
        let selected_char = selection
            .iter()
            .any(|range| absolute >= range.start && absolute < range.end);
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
            } else {
                // A wide glyph straddles `skip`: its first half is scrolled off,
                // so emit the visible half as blanks and keep columns aligned.
                for _ in skip..next.min(limit) {
                    run.push(' ');
                }
            }
        }

        cell = next;
        if cell >= limit {
            break;
        }
    }
    flush(buffer, &mut cursor_x, &mut run, run_style, y);
}

fn in_match(ranges: &[std::ops::Range<usize>], index: &mut usize, absolute: usize) -> bool {
    while *index < ranges.len() && ranges[*index].end <= absolute {
        *index += 1;
    }
    ranges
        .get(*index)
        .is_some_and(|range| range.start <= absolute)
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
