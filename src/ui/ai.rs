//! Right-side AI assistant panel: transcript plus input.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

pub fn render(frame: &mut Frame, state: &AppState) {
    let area = state.ui.ai;
    if area.width < 8 || area.height < 3 {
        return;
    }
    let panel = &state.ai;
    let focused = panel.focused;
    let tokens = crate::ai::context_tokens(&panel.conversation);
    let marker = if panel.busy { " *" } else { "" };
    let title = format!(
        " AI {}/{}{} ",
        compact(tokens),
        compact(state.config.ai.max_context_tokens),
        marker
    );
    let border = Style::default().fg(if focused {
        state.theme.border_active
    } else {
        state.theme.border_inactive
    });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let status = if let Some(call) = panel.pending.front() {
        Some(format!(
            "Allow {}/{} [{}]? [y] yes [n] no",
            call.server, call.tool, call.capability
        ))
    } else if panel.waiting.is_some() {
        Some("tool running...".to_string())
    } else if panel.busy {
        Some("thinking...".to_string())
    } else {
        None
    };
    let input = status
        .is_none()
        .then(|| input_display(&panel.input, panel.cursor, inner.width as usize));
    let max_input_rows = (inner.height / 2).clamp(1, 6) as usize;
    let visible_input_rows = input
        .as_ref()
        .map_or(1, |input| input.rows.len().min(max_input_rows).max(1));
    let transcript_height = inner.height.saturating_sub(visible_input_rows as u16);
    let rows = build_rows(state);
    let max_scroll = rows.len().saturating_sub(transcript_height as usize);
    let scroll = panel.scroll.min(max_scroll) as u16;
    frame.render_widget(
        Paragraph::new(rows).scroll((scroll, 0)),
        Rect::new(inner.x, inner.y, inner.width, transcript_height),
    );

    let input_y = inner.y.saturating_add(transcript_height);
    let style = if focused {
        state.theme.base().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(state.theme.gutter)
    };
    if let Some(status) = status {
        frame
            .buffer_mut()
            .set_stringn(inner.x, input_y, &status, inner.width as usize, style);
    } else if let Some(input) = input {
        let input_start = input
            .cursor_row
            .saturating_sub(visible_input_rows.saturating_sub(1))
            .min(input.rows.len().saturating_sub(visible_input_rows));
        for (offset, row) in input
            .rows
            .iter()
            .skip(input_start)
            .take(visible_input_rows)
            .enumerate()
        {
            frame.buffer_mut().set_stringn(
                inner.x,
                input_y.saturating_add(offset as u16),
                row,
                inner.width as usize,
                style,
            );
        }
        if focused {
            let x = inner
                .x
                .saturating_add(input.cursor_col as u16)
                .min(inner.x.saturating_add(inner.width).saturating_sub(1));
            let y = input_y.saturating_add((input.cursor_row - input_start) as u16);
            frame.set_cursor_position((x, y));
        }
    }
}

struct InputDisplay {
    rows: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

fn input_display(text: &str, cursor: usize, width: usize) -> InputDisplay {
    let width = width.max(3);
    let content_width = width - 2;
    let mut rows = vec!["> ".to_string()];
    let mut row = 0usize;
    let mut cells = 0usize;
    let mut col = 2usize;
    let mut cursor_row = 0usize;
    let mut cursor_col = 2usize;
    let chars: Vec<char> = text.chars().collect();
    for (index, ch) in chars.iter().enumerate() {
        if index == cursor {
            cursor_row = row;
            cursor_col = col;
        }
        if *ch == '\n' {
            rows.push("  ".to_string());
            row += 1;
            cells = 0;
            col = 2;
            continue;
        }
        let ch_width = unicode_width::UnicodeWidthChar::width(*ch).unwrap_or(0);
        if cells > 0 && cells + ch_width > content_width {
            rows.push("  ".to_string());
            row += 1;
            cells = 0;
            col = 2;
        }
        if let Some(row_text) = rows.get_mut(row) {
            row_text.push(*ch);
        }
        cells += ch_width;
        col += ch_width;
    }
    if cursor >= chars.len() {
        cursor_row = row;
        cursor_col = col;
    }
    InputDisplay {
        rows,
        cursor_row,
        cursor_col,
    }
}

pub(crate) fn max_scroll(state: &AppState) -> usize {
    let height = state.ui.ai.height.saturating_sub(3) as usize;
    build_rows(state).len().saturating_sub(height)
}

fn build_rows(state: &AppState) -> Vec<Line<'static>> {
    let width = state.ui.ai.width.saturating_sub(2) as usize;
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut in_code = false;
    for entry in &state.ai.entries {
        push_entry_rows(&mut rows, state, entry, width, &mut in_code);
    }
    rows
}

fn push_entry_rows(
    rows: &mut Vec<Line<'static>>,
    state: &AppState,
    entry: &str,
    width: usize,
    in_code: &mut bool,
) {
    for raw in entry.split('\n') {
        let style = line_style(state, raw, in_code);
        let text = clip(raw, state.ai.scroll_x);
        push_wrapped(rows, &text, width, style);
    }
}

fn push_wrapped(rows: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    if text.is_empty() {
        rows.push(Line::from(""));
        return;
    }
    let mut current = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used > 0 && used + cells > width {
            rows.push(Line::from(Span::styled(
                std::mem::take(&mut current),
                style,
            )));
            used = 0;
        }
        current.push(ch);
        used += cells;
    }
    if !current.is_empty() {
        rows.push(Line::from(Span::styled(current, style)));
    }
}

fn line_style(state: &AppState, line: &str, in_code: &mut bool) -> Style {
    if line.starts_with("```") {
        *in_code = !*in_code;
        return Style::default().fg(state.theme.gutter);
    }
    if *in_code {
        return Style::default().fg(state.theme.gutter);
    }
    if line.starts_with("# ") {
        return Style::default()
            .fg(state.theme.explorer_dir)
            .add_modifier(Modifier::BOLD);
    }
    if line.starts_with("> ") {
        return Style::default().fg(state.theme.border_active);
    }
    if line.starts_with("[error]") || line.starts_with("[blocked]") {
        return state.theme.base().add_modifier(Modifier::BOLD);
    }
    state.theme.base()
}

fn clip(text: &str, skip: usize) -> String {
    if skip == 0 {
        return text.to_string();
    }
    text.chars().skip(skip).collect()
}

fn compact(tokens: usize) -> String {
    if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
