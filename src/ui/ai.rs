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
    let usage = if panel.prompt_tokens + panel.completion_tokens > 0 {
        format!(
            " ↑{} ↓{}",
            compact(panel.prompt_tokens as usize),
            compact(panel.completion_tokens as usize)
        )
    } else {
        String::new()
    };
    let cost = session_cost(state);
    let model = model_label(&state.config.ai.model);
    let title = format!(
        " {model} {}/{}{}{}{} ",
        compact(tokens),
        compact(state.config.ai.max_context_tokens),
        usage,
        cost,
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

pub(crate) fn build_rows(state: &AppState) -> Vec<Line<'static>> {
    let width = state.ui.ai.width.saturating_sub(2) as usize;
    if width == 0 {
        return Vec::new();
    }
    let lines: Vec<String> = state
        .ai
        .entries
        .iter()
        .flat_map(|entry| entry.split('\n'))
        .map(str::to_string)
        .collect();
    let mut rows = Vec::new();
    let mut in_code = false;
    let mut index = 0;
    while index < lines.len() {
        if !in_code && is_table_line(&lines[index]) {
            let start = index;
            index = table_end(&lines, start);
            push_table_rows(&mut rows, state, &lines[start..index], width);
        } else {
            push_markdown_line(&mut rows, state, &lines[index], width, &mut in_code);
            index += 1;
        }
    }
    rows
}

fn is_table_line(line: &str) -> bool {
    let line = line.trim();
    line.len() > 2 && line.starts_with('|') && line.ends_with('|')
}

fn table_end(entries: &[String], start: usize) -> usize {
    entries[start..]
        .iter()
        .take_while(|line| is_table_line(line))
        .count()
        + start
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

fn update_widths(widths: &mut [usize], cells: &[String], state: &AppState) {
    for (column, cell) in cells.iter().enumerate() {
        widths[column] = widths[column].max(cell_width(state, cell));
    }
}

fn cell_width(state: &AppState, cell: &str) -> usize {
    inline_spans(state, cell, state.theme.base())
        .iter()
        .map(|span| display_width(&span.content))
        .sum()
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn split_table_cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_matches('|');
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            cell.push(ch);
            if let Some(next) = chars.next() {
                cell.push(next);
            }
        } else if ch == '|' {
            cells.push(cell.trim().to_string());
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    cells.push(cell.trim().to_string());
    cells
}

fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty()
                && cell.chars().any(|ch| ch == '-')
                && cell.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
        })
}

fn table_align(cell: &str) -> Align {
    let cell = cell.trim();
    match (cell.starts_with(':'), cell.ends_with(':')) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    }
}

fn table_row_spans(
    state: &AppState,
    cells: &[String],
    widths: &[usize],
    aligns: &[Align],
    separator: bool,
    header: bool,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled("│", code_style(state))];
    for (column, width) in widths.iter().enumerate() {
        if separator {
            spans.push(Span::styled("─".repeat(*width), code_style(state)));
        } else {
            let cell = cells.get(column).map(String::as_str).unwrap_or_default();
            let align = aligns.get(column).copied().unwrap_or(Align::Left);
            spans.extend(table_cell_spans(state, cell, *width, align, header));
        }
        spans.push(Span::styled("│", code_style(state)));
    }
    spans
}

fn table_cell_spans(
    state: &AppState,
    cell: &str,
    width: usize,
    align: Align,
    header: bool,
) -> Vec<Span<'static>> {
    let mut base = state.theme.base();
    if header {
        base = base.add_modifier(Modifier::BOLD);
    }
    let mut spans = inline_spans(state, cell, base);
    truncate_spans(&mut spans, width);
    let used: usize = spans.iter().map(|span| display_width(&span.content)).sum();
    let padding = width.saturating_sub(used);
    let (left, right) = match align {
        Align::Left => (0, padding),
        Align::Center => (padding / 2, padding - padding / 2),
        Align::Right => (padding, 0),
    };
    let mut out = Vec::new();
    if left > 0 {
        out.push(Span::styled(" ".repeat(left), base));
    }
    out.extend(spans);
    if right > 0 {
        out.push(Span::styled(" ".repeat(right), base));
    }
    out
}

fn truncate_spans(spans: &mut Vec<Span<'static>>, width: usize) {
    let mut remaining = width;
    let mut out = Vec::new();
    for span in spans.drain(..) {
        if remaining == 0 {
            break;
        }
        let (text, cells) = take_fitting(&span.content, remaining);
        if !text.is_empty() {
            out.push(Span::styled(text, span.style));
            remaining -= cells;
        }
        if remaining == 0 {
            break;
        }
    }
    *spans = out;
}

fn take_fitting(text: &str, limit: usize) -> (String, usize) {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > limit {
            break;
        }
        used += cells;
        out.push(ch);
    }
    (out, used)
}

fn push_table_rows(
    rows: &mut Vec<Line<'static>>,
    state: &AppState,
    block: &[String],
    width: usize,
) {
    let mut aligns: Vec<Align> = Vec::new();
    let parsed: Vec<(Vec<String>, bool)> = block
        .iter()
        .map(|line| {
            let cells = split_table_cells(line);
            let separator = is_separator_row(&cells);
            if separator && aligns.is_empty() {
                aligns = cells.iter().map(|cell| table_align(cell)).collect();
            }
            (cells, separator)
        })
        .collect();
    let columns = parsed
        .iter()
        .map(|(cells, _)| cells.len())
        .max()
        .unwrap_or(0);
    if columns == 0 {
        return;
    }
    aligns.resize(columns, Align::Left);
    let mut widths = vec![1usize; columns];
    for (cells, separator) in &parsed {
        if !separator {
            update_widths(&mut widths, cells, state);
        }
    }
    let border_width = columns + 1;
    let max_cells = width.saturating_sub(border_width) / columns.max(1);
    for width in &mut widths {
        *width = (*width).min(max_cells.max(1));
    }
    for (index, (cells, separator)) in parsed.iter().enumerate() {
        let header =
            !separator && index == 0 && parsed.get(1).is_some_and(|(_, separator)| *separator);
        push_styled(
            rows,
            table_row_spans(state, cells, &widths, &aligns, *separator, header),
            width,
        );
    }
}

fn push_markdown_line(
    rows: &mut Vec<Line<'static>>,
    state: &AppState,
    raw: &str,
    width: usize,
    in_code: &mut bool,
) {
    let text = clip(raw, state.ai.scroll_x);
    if text.starts_with("```") {
        *in_code = !*in_code;
        push_styled(rows, vec![Span::styled(text, code_style(state))], width);
        return;
    }
    if *in_code {
        push_styled(rows, vec![Span::styled(text, code_style(state))], width);
        return;
    }
    let base = state.theme.base();
    let hashes = text.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &text[hashes..];
        if let Some(text) = rest.strip_prefix(' ') {
            let style = base
                .fg(if hashes == 1 {
                    state.theme.explorer_dir
                } else {
                    state.theme.border_active
                })
                .add_modifier(Modifier::BOLD);
            push_styled(rows, inline_spans(state, text, style), width);
            return;
        }
    }
    if text == "---" || text == "***" {
        push_styled(
            rows,
            vec![Span::styled("─".repeat(width), code_style(state))],
            width,
        );
        return;
    }
    let (depth, quoted) = quote_prefix(&text);
    if depth > 0 {
        let mut spans = Vec::new();
        for _ in 0..depth {
            spans.push(Span::styled("│ ", code_style(state)));
        }
        spans.extend(inline_spans(
            state,
            quoted,
            base.fg(state.theme.border_active),
        ));
        push_styled(rows, spans, width);
        return;
    }
    let leading = text.len() - text.trim_start_matches(' ').len();
    let trimmed = &text[leading..];
    if let Some((label, note)) = footnote_definition(trimmed) {
        let mut spans = vec![Span::styled(
            format!("{}[{}] ", " ".repeat(leading), label),
            base.fg(state.theme.gutter),
        )];
        spans.extend(inline_spans(state, note, base));
        push_styled(rows, spans, width);
        return;
    }
    if let Some(text) = trimmed.strip_prefix("- [ ] ") {
        let mut spans = vec![Span::styled(
            format!("{}☐ ", " ".repeat(leading)),
            base.fg(state.theme.explorer_dir),
        )];
        spans.extend(inline_spans(state, text, base));
        push_styled(rows, spans, width);
        return;
    }
    if let Some(text) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        let mut spans = vec![Span::styled(
            format!("{}☑ ", " ".repeat(leading)),
            base.fg(state.theme.explorer_dir),
        )];
        spans.extend(inline_spans(state, text, base));
        push_styled(rows, spans, width);
        return;
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(marker) {
            let mut spans = vec![Span::styled(
                format!("{}• ", " ".repeat(leading)),
                base.fg(state.theme.explorer_dir),
            )];
            spans.extend(inline_spans(state, text, base));
            push_styled(rows, spans, width);
            return;
        }
    }
    if let Some((prefix, text)) = ordered_prefix(trimmed) {
        let mut spans = vec![Span::styled(
            format!("{}{prefix}", " ".repeat(leading)),
            base.fg(state.theme.explorer_dir),
        )];
        spans.extend(inline_spans(state, text, base));
        push_styled(rows, spans, width);
        return;
    }
    push_styled(rows, inline_spans(state, &text, base), width);
}

fn ordered_prefix(text: &str) -> Option<(String, &str)> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = text.get(digits..)?;
    let tail = rest.strip_prefix(". ")?;
    Some((format!("{}. ", &text[..digits]), tail))
}

fn code_style(state: &AppState) -> Style {
    Style::default().fg(state.theme.gutter)
}

fn quote_prefix(text: &str) -> (usize, &str) {
    let mut rest = text.trim_start_matches(' ');
    let mut depth = 0;
    while let Some(after) = rest.strip_prefix('>') {
        depth += 1;
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    (depth, rest)
}

fn footnote_definition(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("[^")?;
    let end = rest.find("]: ")?;
    Some((&rest[..end], &rest[end + 3..]))
}

fn inline_spans(state: &AppState, text: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '\\'
            && let Some(next) = chars.get(index + 1).copied()
            && is_markdown_escape(next)
        {
            buffer.push(next);
            index += 2;
            continue;
        }
        if chars[index] == '*'
            && chars.get(index + 1) == Some(&'*')
            && let Some(end) = find_marker(&chars, index + 2, "**")
        {
            push_text(&mut spans, &mut buffer, base);
            let value: String = chars[index + 2..end].iter().collect();
            spans.push(Span::styled(value, base.add_modifier(Modifier::BOLD)));
            index = end + 2;
            continue;
        }
        if chars[index] == '`'
            && let Some(end) = find_char(&chars, index + 1, '`')
        {
            push_text(&mut spans, &mut buffer, base);
            let value: String = chars[index + 1..index + 1 + end].iter().collect();
            spans.push(Span::styled(value, base.fg(state.theme.syntax.string)));
            index += end + 2;
            continue;
        }
        if chars[index] == '~'
            && chars.get(index + 1) == Some(&'~')
            && let Some(end) = find_marker(&chars, index + 2, "~~")
        {
            push_text(&mut spans, &mut buffer, base);
            let value: String = chars[index + 2..end].iter().collect();
            spans.push(Span::styled(
                value,
                base.add_modifier(Modifier::CROSSED_OUT),
            ));
            index = end + 2;
            continue;
        }
        if chars[index] == '*'
            && let Some(end) = find_char(&chars, index + 1, '*')
        {
            push_text(&mut spans, &mut buffer, base);
            let value: String = chars[index + 1..index + 1 + end].iter().collect();
            spans.push(Span::styled(value, base.add_modifier(Modifier::ITALIC)));
            index += end + 2;
            continue;
        }
        if chars[index] == '['
            && chars.get(index + 1) == Some(&'^')
            && let Some(end) = find_char(&chars, index + 2, ']')
        {
            push_text(&mut spans, &mut buffer, base);
            let label: String = chars[index + 2..index + 2 + end].iter().collect();
            spans.push(Span::styled(
                format!("[{label}]"),
                base.fg(state.theme.gutter),
            ));
            index += end + 3;
            continue;
        }
        if chars[index] == '['
            && let Some((label, next)) = link_at(&chars, index)
        {
            push_text(&mut spans, &mut buffer, base);
            spans.push(Span::styled(
                label,
                base.fg(state.theme.border_active)
                    .add_modifier(Modifier::UNDERLINED),
            ));
            index = next;
            continue;
        }
        buffer.push(chars[index]);
        index += 1;
    }
    push_text(&mut spans, &mut buffer, base);
    spans
}

fn is_markdown_escape(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '|'
            | '>'
            | '~'
    )
}

fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len())
        .find(|index| chars[*index] == needle)
        .map(|index| index - from)
}

fn find_marker(chars: &[char], from: usize, marker: &str) -> Option<usize> {
    let marker: Vec<char> = marker.chars().collect();
    (from..chars.len().saturating_sub(marker.len() - 1))
        .find(|index| chars[*index..*index + marker.len()] == marker)
}

fn link_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let close = chars[start + 1..].iter().position(|ch| *ch == ']')? + start + 1;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|ch| *ch == ')')? + close + 2;
    let label: String = chars[start + 1..close].iter().collect();
    Some((label, end + 1))
}

fn push_text(spans: &mut Vec<Span<'static>>, buffer: &mut String, style: Style) {
    if buffer.is_empty() {
        return;
    }
    spans.push(Span::styled(std::mem::take(buffer), style));
}

fn push_styled(rows: &mut Vec<Line<'static>>, spans: Vec<Span<'static>>, width: usize) {
    if spans.is_empty() {
        rows.push(Line::from(""));
        return;
    }
    let mut current = Vec::new();
    let mut text = String::new();
    let mut style = Style::default();
    let mut used = 0usize;
    for span in spans {
        append_span(
            rows,
            &mut current,
            &mut text,
            &mut style,
            &mut used,
            span,
            width,
        );
    }
    push_text(&mut current, &mut text, style);
    if !current.is_empty() {
        rows.push(Line::from(current));
    }
}

fn append_span(
    rows: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
    text: &mut String,
    style: &mut Style,
    used: &mut usize,
    span: Span<'static>,
    width: usize,
) {
    for ch in span.content.chars() {
        let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if *used > 0 && *used + cells > width {
            push_text(current, text, *style);
            rows.push(Line::from(std::mem::take(current)));
            *used = 0;
        }
        if *style != span.style {
            push_text(current, text, *style);
            *style = span.style;
        }
        text.push(ch);
        *used += cells;
    }
    if !text.is_empty() {
        push_text(current, text, *style);
    }
}

fn clip(text: &str, skip: usize) -> String {
    if skip == 0 {
        return text.to_string();
    }
    text.chars().skip(skip).collect()
}

fn model_label(model: &str) -> String {
    let short = model.rsplit('/').next().unwrap_or(model);
    let mut chars = short.chars();
    let mut label: String = chars.by_ref().take(14).collect();
    if chars.next().is_some() {
        label.pop();
        label.push('…');
    }
    label
}

fn session_cost(state: &AppState) -> String {
    let input = state.config.ai.input_cost_per_million;
    let output = state.config.ai.output_cost_per_million;
    if input <= 0.0 && output <= 0.0 {
        return String::new();
    }
    let cost = (state.ai.prompt_tokens as f64 * input + state.ai.completion_tokens as f64 * output)
        / 1_000_000.0;
    format!(" ${cost:.4}")
}

fn compact(tokens: usize) -> String {
    if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::*;

    #[test]
    fn table_cells_keep_escaped_pipes() {
        let cells = split_table_cells(r"| a \| b | c |");
        assert_eq!(cells, vec![r"a \| b".to_string(), "c".to_string()]);
    }

    #[test]
    fn table_alignment_reads_colons() {
        assert_eq!(table_align("---"), Align::Left);
        assert_eq!(table_align(":---:"), Align::Center);
        assert_eq!(table_align("---:"), Align::Right);
    }

    #[test]
    fn ordered_prefix_allows_indentation() {
        assert_eq!(ordered_prefix("2. item"), Some(("2. ".to_string(), "item")));
        assert_eq!(ordered_prefix("2) item"), None);
    }

    #[test]
    fn markdown_escapes_are_limited_to_punctuation() {
        assert!(is_markdown_escape('*'));
        assert!(is_markdown_escape('|'));
        assert!(!is_markdown_escape('a'));
    }
}
