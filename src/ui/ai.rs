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
    let title = if panel.busy {
        " AI * ".to_string()
    } else {
        " AI ".to_string()
    };
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
    let rows: Vec<Line> = panel
        .entries
        .iter()
        .skip(scroll)
        .take(transcript_height as usize)
        .map(|line| Line::from(Span::styled(line.clone(), state.theme.base())))
        .collect();
    frame.render_widget(
        Paragraph::new(rows),
        Rect::new(inner.x, inner.y, inner.width, transcript_height),
    );

    let footer = if let Some(call) = panel.pending.front() {
        format!("Allow {}/{}? [y] yes [n] no", call.server, call.tool)
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
