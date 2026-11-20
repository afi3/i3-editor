//! Rendering. Every function here takes `&AppState` and never mutates it.

pub mod overlay;
pub mod pane;
pub mod shell;

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::state::{AppState, InputMode, PaneId};

#[derive(Debug, Clone)]
pub struct TabHit {
    pub label: String,
    pub start: u16,
    pub end: u16,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct TabBar {
    pub path: Vec<usize>,
    pub rect: Rect,
    pub tabs: Vec<TabHit>,
}

#[derive(Debug, Default)]
pub struct UiState {
    pub area: Rect,
    pub pane_rects: HashMap<PaneId, Rect>,
    pub workspace_bar: Rect,
    pub explorer: Rect,
    pub panes: Rect,
    pub shell: Rect,
    pub status: Rect,
    pub message: String,
    pub tab_bars: Vec<TabBar>,
}

/// Pane rectangles for the current layout. A fullscreen pane replaces the whole
/// area, so rendering, mouse hits, and scrolling all agree on what is visible.
pub fn layout_rects(state: &AppState, area: Rect) -> HashMap<PaneId, Rect> {
    if let Some(id) = state.fullscreen
        && state.workspaces.active().layout.contains(id)
    {
        return HashMap::from([(id, area)]);
    }
    let mut rects = HashMap::new();
    state.workspaces.active().layout.layout(area, &mut rects);
    rects
}

fn tab_bars(state: &AppState, area: Rect) -> Vec<TabBar> {
    let mut strips = Vec::new();
    state
        .workspaces
        .active()
        .layout
        .tab_strips(area, &mut Vec::new(), &mut strips);
    let mut out = Vec::with_capacity(strips.len());
    for strip in strips {
        if strip.rect.width == 0 || strip.rect.height == 0 {
            continue;
        }
        let right = strip.rect.x.saturating_add(strip.rect.width);
        let mut start = strip.rect.x;
        let mut tabs = Vec::with_capacity(strip.leaves.len());
        for (index, leaf) in strip.leaves.iter().enumerate() {
            if start >= right {
                break;
            }
            let leaves = strip.counts.get(index).copied().unwrap_or(1);
            let label = tab_label(state, *leaf, leaves);
            let width = crate::editor::text::cells(&label, 4, 0).saturating_add(4);
            let end = start.saturating_add(width).min(right);
            tabs.push(TabHit {
                label,
                start,
                end,
                active: index == strip.active,
            });
            start = end;
        }
        out.push(TabBar {
            path: strip.path,
            rect: strip.rect,
            tabs,
        });
    }
    out
}

fn tab_label(state: &AppState, leaf: PaneId, leaves: usize) -> String {
    let name = state
        .panes
        .get(&leaf)
        .and_then(|pane| pane.active)
        .and_then(|id| state.documents.get(&id))
        .map(|document| {
            format!(
                "{}{}",
                document.display_name(),
                if document.dirty { "*" } else { "" }
            )
        })
        .unwrap_or_else(|| format!("pane {leaf}"));
    if leaves > 1 {
        format!("{name} +{}", leaves - 1)
    } else {
        name
    }
}

/// Recompute everything that depends on the pane area: rectangles and tab strips.
pub fn refresh_panes(state: &mut AppState) {
    let area = state.ui.panes;
    let rects = layout_rects(state, area);
    let bars = if state.fullscreen.is_some() {
        Vec::new()
    } else {
        tab_bars(state, area)
    };
    state.ui.pane_rects = rects;
    state.ui.tab_bars = bars;
}

/// Recompute the region map once per frame, before drawing.
pub fn compute_layout(state: &mut AppState, area: Rect) {
    let workspace_height = if state.config.appearance.show_workspace_tabs && area.height > 3 {
        1
    } else {
        0
    };
    let status_height = if state.config.appearance.show_status_bar && area.height > 2 {
        1
    } else {
        0
    };
    let main_height = area.height.saturating_sub(workspace_height + status_height);
    let shell_height = if state.shell.is_some() {
        let maximum = main_height.saturating_sub(crate::workspace::MIN_PANE_ROWS + 1);
        state.config.terminal.height.min(maximum)
    } else {
        0
    };
    let body_height = main_height.saturating_sub(shell_height);

    let workspace_bar = Rect::new(area.x, area.y, area.width, workspace_height);
    let main = Rect::new(
        area.x,
        area.y.saturating_add(workspace_height),
        area.width,
        body_height,
    );
    let shell = Rect::new(
        area.x,
        area.y.saturating_add(workspace_height + body_height),
        area.width,
        shell_height,
    );
    let status = Rect::new(
        area.x,
        area.y.saturating_add(workspace_height + main_height),
        area.width,
        status_height,
    );

    let explorer_width = if state.explorer.visible {
        (u32::from(area.width) * u32::from(state.config.explorer.width) / 100) as u16
    } else {
        0
    };
    let explorer = Rect::new(main.x, main.y, explorer_width.min(main.width), main.height);
    let panes = Rect::new(
        main.x.saturating_add(explorer.width),
        main.y,
        main.width.saturating_sub(explorer.width),
        main.height,
    );

    state.ui.area = area;
    state.ui.panes = panes;
    state.ui.shell = shell;
    refresh_panes(state);
    state.ui.workspace_bar = workspace_bar;
    state.ui.explorer = explorer;
    state.ui.status = status;
}

pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    frame.buffer_mut().set_style(area, state.theme.base());

    if state.ui.workspace_bar.height > 0 {
        render_workspace_bar(frame, state);
    }

    if state.explorer.visible && state.ui.explorer.width > 0 {
        overlay::render_explorer(frame, state);
    }

    let mut panes: Vec<(PaneId, Rect)> = state
        .ui
        .pane_rects
        .iter()
        .map(|(id, rect)| (*id, *rect))
        .collect();
    panes.sort_by_key(|(id, rect)| (rect.y, rect.x, id.0));
    for (pane_id, rect) in panes {
        let Some(pane) = state.panes.get(&pane_id) else {
            continue;
        };
        let document = pane.active.and_then(|id| state.documents.get(&id));
        pane::render(frame, state, pane, document, rect);
    }

    render_tab_bars(frame, state);

    if state.ui.shell.height > 0 {
        shell::render(frame, state);
    }

    if state.ui.status.height > 0 {
        render_status(frame, state);
    }
    if let InputMode::Prompt(prompt) = &state.mode {
        overlay::render_prompt(frame, state, prompt);
    }
}

fn render_tab_bars(frame: &mut Frame, state: &AppState) {
    for bar in &state.ui.tab_bars {
        frame.buffer_mut().set_style(bar.rect, state.theme.base());
        for tab in &bar.tabs {
            if tab.end <= tab.start {
                continue;
            }
            let width = tab.end - tab.start;
            let style = if tab.active {
                state.theme.tab_active_style()
            } else {
                state.theme.tab_inactive_style()
            };
            let avail = width.saturating_sub(2);
            let body = crate::editor::text::clip(&tab.label, 0, avail, 4);
            let buffer = frame.buffer_mut();
            buffer.set_stringn(
                tab.start,
                bar.rect.y,
                format!(" {body} "),
                width as usize,
                style,
            );
        }
    }
}

fn render_workspace_bar(frame: &mut Frame, state: &AppState) {
    let mut spans = Vec::new();
    for (index, workspace) in state.workspaces.workspaces.iter().enumerate() {
        let style = if index == state.workspaces.active {
            state.theme.tab_active_style()
        } else {
            state.theme.tab_inactive_style()
        };
        spans.push(Span::styled(
            format!(" {}:{} ", index + 1, workspace.name),
            style,
        ));
    }
    spans.push(Span::styled(
        format!(
            "  F7 new  F8 close  ({} panes)",
            state.workspaces.active().layout.leaves().len()
        ),
        Style::default()
            .bg(state.theme.background)
            .fg(state.theme.gutter),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), state.ui.workspace_bar);
}

fn render_status(frame: &mut Frame, state: &AppState) {
    let document = state.active_document();
    let pane = state.focused_pane();

    let name = match document {
        Some(document) => format!(
            "{}{}",
            document.display_name(),
            if document.dirty { " [+]" } else { "" }
        ),
        None => "-".to_string(),
    };

    let (line, column, total) = match (pane, document) {
        (Some(pane), Some(document)) => {
            let line = document.line_of_char(pane.cursor) + 1;
            let line_start = document.char_of_line(line - 1);
            let column = pane.cursor.saturating_sub(line_start) + 1;
            (line, column, document.line_count())
        }
        _ => (0, 0, 0),
    };

    let focus = match state.focus {
        crate::state::Focus::Explorer => "EXPLORER".to_string(),
        crate::state::Focus::Pane(id) => format!("PANE {id}"),
        crate::state::Focus::Shell => "SHELL".to_string(),
    };
    let encoding = document
        .map(|document| document.eol.as_str())
        .unwrap_or("-");

    let left = format!(" {name} | {line}:{column} | {total} lines | UTF-8 {encoding} | {focus} ");
    let left = if state.moving {
        format!("{left} | MOVE: arrows move the pane, Esc exits ")
    } else {
        left
    };

    let message = if state.ui.message.is_empty() {
        String::new()
    } else {
        format!("  {}", state.ui.message)
    };

    let line = Line::from(vec![
        Span::styled(left, state.theme.status_style()),
        Span::styled(message, state.theme.status_style()),
    ]);
    frame.render_widget(Paragraph::new(line), state.ui.status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::editor::document::{Document, Eol};
    use crate::editor::history::History;
    use crate::explorer::ExplorerState;
    use crate::input::{KeyMap, PromptKind, PromptState};
    use crate::state::{DocumentId, Focus, Ids, InputMode, Pane, Search, WorkspaceId};
    use crate::workspace::{SplitAxis, Workspace, WorkspaceManager};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn state(text: &str) -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "i3-render-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pane_id = PaneId(1);
        let document_id = DocumentId(1);
        let mut panes = HashMap::new();
        let mut pane = Pane::new(pane_id);
        pane.tabs.push(document_id);
        pane.active = Some(document_id);
        panes.insert(pane_id, pane);

        let mut documents = HashMap::new();
        documents.insert(
            document_id,
            Document {
                id: document_id,
                path: Some(dir.join("main.rs")),
                rope: ropey::Rope::from_str(text),
                dirty: false,
                eol: Eol::Lf,
                history: History::default(),
                syntax: Default::default(),
            },
        );

        let mut workspaces = WorkspaceManager::default();
        workspaces.push(Workspace::new(WorkspaceId(1), "test", pane_id));

        let state = AppState {
            should_quit: false,
            explorer: ExplorerState::new(dir.clone(), false).expect("explorer"),
            documents,
            panes,
            workspaces,
            focus: Focus::Pane(pane_id),
            mode: InputMode::Normal,
            keymap: KeyMap::default(),
            ui: UiState::default(),
            config: AppConfig::default(),
            theme: crate::theme::Theme::default(),
            ids: Ids::default(),
            warnings: Vec::new(),
            clipboard: String::new(),
            search: None,
            fullscreen: None,
            moving: false,
            shell: None,
            themes_dir: std::path::PathBuf::new(),
            config_path: None,
        };
        (state, dir)
    }

    fn draw(state: &mut AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                compute_layout(state, frame.area());
                render(frame, state);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_status_bar_and_document() {
        let (mut state, dir) = state("hello\nworld\n");
        let screen = draw(&mut state, 80, 24);
        assert!(screen.contains("main.rs"), "{screen}");
        assert!(screen.contains("1:1"), "{screen}");
        assert!(screen.contains("3 lines"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_every_pane_in_a_nested_split() {
        let (mut state, dir) = state("alpha\nbeta\n");
        let second = PaneId(2);
        let mut pane = Pane::new(second);
        pane.tabs.push(DocumentId(1));
        pane.active = Some(DocumentId(1));
        state.panes.insert(second, pane);
        state.workspaces.active_mut().layout.split_pane(
            PaneId(1),
            crate::workspace::SplitAxis::LeftRight,
            second,
        );

        let screen = draw(&mut state, 80, 24);
        assert_eq!(state.ui.pane_rects.len(), 2);
        for rect in state.ui.pane_rects.values() {
            assert!(rect.width >= crate::workspace::MIN_PANE_COLS);
            assert!(rect.height >= crate::workspace::MIN_PANE_ROWS);
        }
        assert!(screen.matches("main.rs").count() >= 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stays_inside_a_tiny_terminal() {
        let (mut state, dir) = state("x\n");
        let _screen = draw(&mut state, 20, 6);
        for rect in state.ui.pane_rects.values() {
            assert!(rect.x + rect.width <= 20);
            assert!(rect.y + rect.height <= 6);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    fn draw_buffer(state: &mut AppState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                compute_layout(state, frame.area());
                render(frame, state);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn workspace_bar_uses_theme_colors() {
        use crate::theme::Theme;
        let (mut state, dir) = state("abc\n");
        let theme = Theme::preset("dark");
        state.theme = theme;
        let buffer = draw_buffer(&mut state, 80, 24);
        let cell = &buffer[(1, state.ui.workspace_bar.y)];
        assert_eq!(cell.bg, theme.tab_active, "active workspace tab bg");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn applies_theme_colors_to_text_and_current_line() {
        use crate::theme::Theme;

        let (mut state, dir) = state("alpha\nbeta\n");
        let theme = Theme::preset("dark");
        state.theme = theme;
        let pane_id = PaneId(1);
        state.focus = Focus::Pane(pane_id);
        state.panes.get_mut(&pane_id).expect("pane").cursor = 0;

        let buffer = draw_buffer(&mut state, 80, 24);
        let rect = state.ui.pane_rects[&pane_id];
        let geometry = {
            let pane = state.panes.get(&pane_id).expect("pane");
            let document = state.documents.get(&DocumentId(1)).expect("document");
            crate::ui::pane::text_geometry(&state, pane, document, rect).expect("geometry")
        };

        let current = &buffer[(geometry.area.x + geometry.gutter, geometry.area.y)];
        assert_eq!(current.bg, theme.current_line, "cursor line background");
        assert_eq!(current.fg, theme.foreground, "cursor line text");

        let second = &buffer[(geometry.area.x + geometry.gutter, geometry.area.y + 1)];
        assert_eq!(
            second.bg, theme.background,
            "other lines use the theme background"
        );

        let border = &buffer[(rect.x, rect.y)];
        assert_eq!(border.fg, theme.border_active, "focused pane border");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompt_uses_theme_colors() {
        use crate::theme::Theme;

        let (mut state, dir) = state("abc\n");
        let theme = Theme::preset("light");
        state.theme = theme;
        state.mode = InputMode::Prompt(PromptState::new(PromptKind::Find));

        let buffer = draw_buffer(&mut state, 80, 24);
        let found = (0..24)
            .flat_map(|row| (0..80).map(move |column| (column, row)))
            .find(|(column, row)| buffer[(*column, *row)].fg == theme.prompt_border);
        assert!(found.is_some(), "prompt border color not found");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn syntax_colors_tokens_in_the_pane() {
        use crate::theme::Theme;

        let (mut state, dir) = state("let answer = 42;\n");
        let theme = Theme::preset("dark");
        state.theme = theme;
        state.focus = Focus::Pane(PaneId(1));

        let buffer = draw_buffer(&mut state, 80, 24);
        let rect = state.ui.pane_rects[&PaneId(1)];
        let geometry = {
            let pane = state.panes.get(&PaneId(1)).expect("pane");
            let document = state.documents.get(&DocumentId(1)).expect("document");
            crate::ui::pane::text_geometry(&state, pane, document, rect).expect("geometry")
        };
        let x = geometry.area.x + geometry.gutter;
        let y = geometry.area.y;

        assert_eq!(buffer[(x, y)].symbol(), "l");
        assert_eq!(buffer[(x, y)].fg, theme.syntax.keyword, "keyword");
        assert_eq!(buffer[(x + 4, y)].fg, theme.foreground, "identifier");
        assert_eq!(buffer[(x + 13, y)].fg, theme.syntax.number, "number");
        assert_eq!(
            buffer[(x + 15, y)].fg,
            theme.syntax.punctuation,
            "punctuation"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multiline_comments_stay_colored_on_later_lines() {
        use crate::theme::Theme;

        let (mut state, dir) = state("/* start\nstill comment */ let x = 1;\n");
        let theme = Theme::preset("dark");
        state.theme = theme;

        let buffer = draw_buffer(&mut state, 80, 24);
        let rect = state.ui.pane_rects[&PaneId(1)];
        let geometry = {
            let pane = state.panes.get(&PaneId(1)).expect("pane");
            let document = state.documents.get(&DocumentId(1)).expect("document");
            crate::ui::pane::text_geometry(&state, pane, document, rect).expect("geometry")
        };
        let x = geometry.area.x + geometry.gutter;
        let y = geometry.area.y;

        assert_eq!(buffer[(x, y + 1)].symbol(), "s");
        assert_eq!(
            buffer[(x, y + 1)].fg,
            theme.syntax.comment,
            "continuation line stays a comment"
        );
        assert_eq!(buffer[(x + 17, y + 1)].symbol(), "l");
        assert_eq!(
            buffer[(x + 17, y + 1)].fg,
            theme.syntax.keyword,
            "text after the closing marker is highlighted again"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_container_tab_strip() {
        let (mut state, dir) = state("abc\n");
        let second = PaneId(2);
        let mut pane = Pane::new(second);
        pane.tabs.push(DocumentId(1));
        pane.active = Some(DocumentId(1));
        state.panes.insert(second, pane);
        state
            .workspaces
            .active_mut()
            .layout
            .split_pane(PaneId(1), SplitAxis::LeftRight, second);
        assert!(
            state
                .workspaces
                .active_mut()
                .layout
                .toggle_tabbed(PaneId(1))
        );

        let screen = draw(&mut state, 80, 24);
        assert_eq!(state.ui.tab_bars.len(), 1);
        assert_eq!(state.ui.tab_bars[0].tabs.len(), 2);
        assert!(screen.contains("main.rs"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrite_prompt_names_the_target_path() {
        let (mut state, dir) = state("abc\n");
        let mut prompt = PromptState::new(PromptKind::Overwrite);
        prompt.target = Some(dir.join("other.txt"));
        state.mode = InputMode::Prompt(prompt);
        let screen = draw(&mut state, 80, 24);
        assert!(screen.contains("Overwrite"), "{screen}");
        assert!(screen.contains("other.txt"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_quit_prompt_options() {
        let (mut state, dir) = state("abc\n");
        let mut prompt = PromptState::new(PromptKind::QuitDirty);
        prompt.set_candidates(vec![
            "Save all and quit".to_string(),
            "Discard all and quit".to_string(),
        ]);
        state.mode = InputMode::Prompt(prompt);
        let screen = draw(&mut state, 80, 24);
        assert!(screen.contains("Save all and quit"), "{screen}");
        assert!(screen.contains("Discard all and quit"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_move_mode_hint() {
        let (mut state, dir) = state("abc\n");
        state.moving = true;
        let screen = draw(&mut state, 100, 24);
        assert!(screen.contains("MOVE"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_prompt_overlay() {
        let (mut state, dir) = state("abc\n");
        state.mode = InputMode::Prompt(PromptState::new(PromptKind::Find));
        let screen = draw(&mut state, 80, 24);
        assert!(screen.contains("Find"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_prompt_lists_matches_and_highlights_all_of_them() {
        use ratatui::style::Modifier;

        let (mut state, dir) = state("alpha\nbeta alpha\n");
        let theme = crate::theme::Theme::default();
        state.theme = theme;
        state.search = Some(Search {
            document: DocumentId(1),
            query: "alpha".to_string(),
            matches: vec![0, 11],
            index: 1,
        });
        let mut prompt = PromptState::new(PromptKind::Find);
        prompt = prompt.with_field("alpha");
        state.mode = InputMode::Prompt(prompt);

        let buffer = draw_buffer(&mut state, 80, 24);
        {
            let pane = state.panes.get(&PaneId(1)).expect("pane");
            let document = state.documents.get(&DocumentId(1)).expect("document");
            let rect = state.ui.pane_rects[&PaneId(1)];
            let geometry =
                super::pane::text_geometry(&state, pane, document, rect).expect("geometry");
            let cell = &buffer[(geometry.area.x + geometry.gutter, geometry.area.y)];
            assert_eq!(cell.symbol(), "a");
            assert_eq!(cell.bg, theme.selection);
            assert!(cell.modifier.contains(Modifier::UNDERLINED));
        }

        let screen = draw(&mut state, 80, 24);
        assert!(screen.contains("Find 2/2"), "{screen}");
        assert!(screen.contains("    2  beta alpha"), "{screen}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
