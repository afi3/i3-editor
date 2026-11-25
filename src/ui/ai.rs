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
    let input_row = inner.y.saturating_add(inner.height.saturating_sub(1));
    let transcript_height = inner.height.saturating_sub(1);
    let last = panel.entries.len().saturating_sub(1);
    let scroll = panel.scroll.min(last);
    let mut in_code = false;
    for line in panel.entries.iter().take(scroll) {
        if line.starts_with("```") {
            in_code = !in_code;
        }
    }
    let rows: Vec<Line> = panel
        .entries
        .iter()
        .skip(scroll)
        .take(transcript_height as usize)
        .map(|line| {
            let style = line_style(state, line, &mut in_code);
            let text = clip(line, panel.scroll_x);
            Line::from(Span::styled(text, style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(rows),
        Rect::new(inner.x, inner.y, inner.width, transcript_height),
    );

    let footer = if let Some(call) = panel.pending.front() {
        format!(
            "Allow {}/{} [{}]? [y] yes [n] no",
            call.server, call.tool, call.capability
        )
    } else if panel.waiting.is_some() {
        "tool running...".to_string()
    } else if panel.busy {
        "thinking...".to_string()
    } else {
        format!("> {}", panel.input)
    };
    let style = if focused {
        state.theme.base().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(state.theme.gutter)
    };
    frame
        .buffer_mut()
        .set_stringn(inner.x, input_row, &footer, inner.width as usize, style);
    if focused && panel.pending.is_empty() && panel.waiting.is_none() && !panel.busy {
        let cursor = panel.cursor.min(panel.input.chars().count());
        let x = inner
            .x
            .saturating_add(2)
            .saturating_add(cursor as u16)
            .min(inner.x.saturating_add(inner.width).saturating_sub(1));
        frame.set_cursor_position((x, input_row));
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
