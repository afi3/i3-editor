//! Bottom terminal panel rendering.

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

use crate::state::{AppState, Focus};
use crate::theme::Theme;

pub fn render(frame: &mut Frame, state: &AppState) {
    let Some(shell) = &state.shell else {
        return;
    };
    let area = state.ui.shell;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = state.focus == Focus::Shell;
    let theme = state.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            theme.border_active
        } else {
            theme.border_inactive
        }))
        .style(theme.base())
        .title(format!(" Terminal — {} ", shell.title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let buffer = frame.buffer_mut();
    buffer.set_style(inner, theme.base());
    let screen = shell.screen();
    for row in 0..inner.height {
        for col in 0..inner.width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let x = inner.x.saturating_add(col);
            let y = inner.y.saturating_add(row);
            let style = cell_style(&theme, cell);
            let contents = cell.contents();
            if contents.is_empty() {
                buffer.set_stringn(x, y, " ", 1, style);
            } else {
                buffer.set_stringn(x, y, contents, (inner.width - col) as usize, style);
            }
        }
    }

    if focused {
        let (row, col) = shell.cursor();
        if row < inner.height && col < inner.width {
            frame.set_cursor_position((inner.x.saturating_add(col), inner.y.saturating_add(row)));
        }
    }
}

fn cell_style(theme: &Theme, cell: &vt100::Cell) -> Style {
    let mut foreground = vt_color(cell.fgcolor(), theme.foreground);
    let mut background = vt_color(cell.bgcolor(), theme.background);
    if cell.inverse() {
        std::mem::swap(&mut foreground, &mut background);
    }
    let mut style = Style::default().fg(foreground).bg(background);
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

fn vt_color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}
