//! Explorer and prompt overlays.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

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
    if Some(search.document) != state.active_document_id()
        || search.revision != document.revision
        || prompt.input() != search.query
    {
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
        .map(|range| {
            let line = document.line_of_char(range.start);
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
    let title = prompt_title(state, prompt);
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
        let label = field_label(prompt, index);
        let label_width = textutil::cells(&label, tab_width, 0);
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

    render_candidates(frame, state, prompt, inner, fields, candidates, tab_width);

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

#[allow(clippy::too_many_arguments)]
fn render_candidates(
    frame: &mut Frame,
    state: &AppState,
    prompt: &PromptState,
    inner: Rect,
    fields: u16,
    candidates: u16,
    tab_width: usize,
) {
    if candidates == 0 {
        return;
    }
    let theme = state.theme;
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
            let text_x = textutil::cells(candidate, tab_width, 0).saturating_add(2);
            let hint = state
                .keymap
                .active_binding(state.focus, &command)
                .map(|label| format!("[{label}]"));
            let hint_width = hint
                .as_ref()
                .map_or(0, |hint| textutil::cells(hint, tab_width, 0));
            if text_x < inner.width {
                let description_width = inner
                    .width
                    .saturating_sub(text_x)
                    .saturating_sub(hint_width.saturating_add(1));
                buffer.set_stringn(
                    inner.x + text_x,
                    row,
                    command.description(),
                    description_width as usize,
                    Style::default().add_modifier(Modifier::DIM),
                );
            }
            if let Some(hint) = hint
                && hint_width < inner.width
            {
                buffer.set_stringn(
                    inner.x + inner.width - hint_width,
                    row,
                    &hint,
                    hint_width as usize,
                    Style::default().fg(theme.gutter),
                );
            }
        }
    }
}

pub fn render_popups(frame: &mut Frame, state: &AppState) {
    if let Some(popup) = &state.completions {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(70);
        let height = (popup.items.len().min(9) as u16)
            .saturating_add(2)
            .min(area.height);
        if width > 0 && height > 0 {
            let rect = Rect::new(
                area.x + (area.width.saturating_sub(width)) / 2,
                area.y + area.height.saturating_sub(height.saturating_add(3)),
                width,
                height,
            );
            frame.render_widget(Clear, rect);
            let rows: Vec<Line> = popup
                .items
                .iter()
                .enumerate()
                .take(9)
                .map(|(index, item)| {
                    let style = if index == popup.selected {
                        state.theme.selection_style()
                    } else {
                        state.theme.base()
                    };
                    Line::from(vec![
                        Span::styled(format!(" {}", item.label), style),
                        Span::styled(
                            format!("  {}", item.detail),
                            Style::default().add_modifier(Modifier::DIM),
                        ),
                    ])
                })
                .collect();
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(state.theme.prompt_border))
                .title(" Completion ");
            frame.render_widget(Paragraph::new(rows).block(block), rect);
        }
    }
    if let Some(text) = &state.hover {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(80);
        let height = (text.lines().count().clamp(1, 10) as u16)
            .saturating_add(2)
            .min(area.height);
        if width > 0 && height > 0 {
            let rect = Rect::new(
                area.x + (area.width.saturating_sub(width)) / 2,
                area.y + area.height.saturating_sub(height.saturating_add(3)),
                width,
                height,
            );
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(state.theme.prompt_border))
                .title(" Hover ");
            frame.render_widget(
                Paragraph::new(text.as_str())
                    .block(block)
                    .wrap(Wrap { trim: false }),
                rect,
            );
        }
    }
}

pub fn render_diff(frame: &mut Frame, state: &AppState) {
    let Some(view) = &state.git_diff else {
        return;
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4);
    let height = area.height.saturating_sub(4);
    if width < 10 || height < 3 {
        return;
    }
    let rect = Rect::new(area.x + 2, area.y + 2, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.prompt_border))
        .title(view.title.clone());
    let inner = block.inner(rect);
    let rows: Vec<Line> = view
        .lines
        .iter()
        .skip(view.scroll)
        .take(inner.height as usize)
        .map(|line| {
            let style = if line.starts_with("+++") || line.starts_with("---") {
                Style::default().add_modifier(Modifier::DIM)
            } else if line.starts_with('+') {
                Style::default().fg(ratatui::style::Color::Green)
            } else if line.starts_with('-') {
                Style::default().fg(ratatui::style::Color::Red)
            } else if line.starts_with("@@") {
                Style::default().fg(ratatui::style::Color::Cyan)
            } else {
                state.theme.base()
            };
            Line::from(Span::styled(line.clone(), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}

pub fn render_help(frame: &mut Frame, state: &AppState) {
    let Some(view) = &state.help else {
        return;
    };
    let area = frame.area();
    let width = ((u32::from(area.width) * 80 / 100) as u16).clamp(40, 110);
    let width = width.min(area.width.max(1));
    let height = area.height.saturating_sub(4).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.prompt_border))
        .title(" Keybindings ");
    let inner = block.inner(rect);
    let rows: Vec<Line> = view
        .lines
        .iter()
        .skip(view.scroll)
        .take(inner.height as usize)
        .map(|line| {
            let style = if line.is_empty() || !line.contains(' ') {
                state.theme.base().add_modifier(Modifier::BOLD)
            } else {
                state.theme.base()
            };
            Line::from(Span::styled(line.clone(), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}

pub fn render_mcp(frame: &mut Frame, state: &AppState) {
    render_read_only(frame, state, state.mcp_view.as_ref(), " MCP ");
}

pub fn render_rules(frame: &mut Frame, state: &AppState) {
    render_read_only(frame, state, state.rules_view.as_ref(), " Workspace rules ");
}

pub fn render_trace(frame: &mut Frame, state: &AppState) {
    render_read_only(frame, state, state.trace_view.as_ref(), " AI trace ");
}

fn render_read_only(
    frame: &mut Frame,
    state: &AppState,
    view: Option<&crate::state::HelpView>,
    title: &str,
) {
    let Some(view) = view else {
        return;
    };
    let area = frame.area();
    let width = ((u32::from(area.width) * 80 / 100) as u16).clamp(40, 110);
    let width = width.min(area.width.max(1));
    let height = area.height.saturating_sub(4).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.prompt_border))
        .title(title);
    let inner = block.inner(rect);
    let rows: Vec<Line> = view
        .lines
        .iter()
        .skip(view.scroll)
        .take(inner.height as usize)
        .map(|line| Line::from(Span::styled(line.clone(), state.theme.base())))
        .collect();
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}

pub fn render_hunks(frame: &mut Frame, state: &AppState) {
    let Some(picker) = &state.hunks else {
        return;
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4);
    let height = area.height.saturating_sub(4);
    if width < 10 || height < 3 {
        return;
    }
    let rect = Rect::new(area.x + 2, area.y + 2, width, height);
    frame.render_widget(Clear, rect);
    let title = format!(
        "{} [{}/{}]  Up/Down hunk, wheel/PgUp/PgDn scroll, Enter applies, Esc closes ",
        picker.title,
        picker.selected + 1,
        picker.hunks.len()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.prompt_border))
        .title(title);
    let inner = block.inner(rect);
    let Some(hunk) = picker.hunks.get(picker.selected) else {
        return;
    };
    let rows: Vec<Line> = hunk
        .lines
        .iter()
        .skip(picker.scroll)
        .take(inner.height as usize)
        .map(|line| {
            let style = if line.starts_with('+') {
                Style::default().fg(ratatui::style::Color::Green)
            } else if line.starts_with('-') {
                Style::default().fg(ratatui::style::Color::Red)
            } else if line.starts_with("@@") {
                Style::default().fg(ratatui::style::Color::Cyan)
            } else {
                state.theme.base()
            };
            Line::from(Span::styled(line.clone(), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}

fn prompt_title(state: &AppState, prompt: &PromptState) -> String {
    let global = flags_suffix(state.search_options);
    let query = prompt.fields.first().map(String::as_str).unwrap_or("");
    match prompt.kind {
        PromptKind::FindInFiles | PromptKind::ReplaceInFiles => {
            let (matches, files, truncated, options) = state.project_search.as_ref().map_or(
                (0, 0, false, state.search_options),
                |search| {
                    (
                        search.results.len(),
                        crate::project_search::group_by_file(&search.results).len(),
                        search.truncated,
                        search.options,
                    )
                },
            );
            let own = flags_suffix(options);
            let truncated = if truncated { "+" } else { "" };
            if prompt.kind == PromptKind::ReplaceInFiles {
                format!(" Replace in Files {matches}{truncated} in {files} files{own} ")
            } else {
                format!(" Find in Files {matches}{truncated}{own} ")
            }
        }
        PromptKind::Find => match state.search.as_ref() {
            Some(search)
                if Some(search.document) == state.active_document_id()
                    && search.query == query
                    && !search.matches.is_empty() =>
            {
                let truncated = if search.truncated { "+" } else { "" };
                format!(
                    " Find{global} {}/{}{truncated} ",
                    search.index + 1,
                    search.matches.len()
                )
            }
            _ => format!(" Find{global} "),
        },
        PromptKind::Replace => {
            let counts = state
                .search
                .as_ref()
                .filter(|search| {
                    search.query == query && Some(search.document) == state.active_document_id()
                })
                .map(|search| (search.matches.len(), search.truncated));
            match counts {
                Some((total, truncated)) => format!(
                    " Replace{global} {total}{} ",
                    if truncated { "+" } else { "" }
                ),
                None => format!(" Replace{global} "),
            }
        }
        _ => format!(" {} ", prompt.kind.title()),
    }
}

fn flags_suffix(options: crate::search_options::Options) -> String {
    let flags = options.flags();
    if flags.is_empty() {
        String::new()
    } else {
        format!(" [{flags}]")
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

fn field_label(prompt: &PromptState, index: usize) -> String {
    if prompt.kind == PromptKind::McpArgs
        && let Some(arg) = prompt.mcp_fields.get(index)
    {
        return format!("{}{}: ", arg.name, if arg.required { "*" } else { "" });
    }
    match (prompt.kind, index) {
        (PromptKind::Replace | PromptKind::ReplaceInFiles, 0) => "Find: ".to_string(),
        (PromptKind::Replace | PromptKind::ReplaceInFiles, 1) => "With: ".to_string(),
        (PromptKind::Find | PromptKind::FindInFiles, _) => "Find: ".to_string(),
        (PromptKind::GoToLine, _) => "Line: ".to_string(),
        (PromptKind::WorkspaceRoot, _) => "Root: ".to_string(),
        (PromptKind::McpAddPath, _) => "Directory: ".to_string(),
        (PromptKind::McpPromptArgs, index) => format!(
            "{}: ",
            prompt
                .mcp_prompt_args
                .get(index)
                .map(String::as_str)
                .unwrap_or("argument")
        ),
        (PromptKind::CommandPalette, _) => "> ".to_string(),
        _ => String::new(),
    }
}
