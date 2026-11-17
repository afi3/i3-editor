//! End-to-end tests for command dispatch, without a real terminal.

use std::path::PathBuf;

use crate::command::Command;
use crate::config::AppConfig;
use crate::editor::document::{Document, Eol};
use crate::editor::history::History;
use crate::explorer::ExplorerState;
use crate::input::KeyMap;
use crate::state::{AppState, Focus, Ids, InputMode, Pane};
use crate::ui;
use crate::workspace::{SplitAxis, Workspace, WorkspaceManager};

use super::App;

fn fixture(text: &str) -> (App, PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("i3-app-{}-{}", std::process::id(), unique));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("note.txt");
    std::fs::write(&path, text).expect("seed file");

    let mut ids = Ids::default();
    let pane_id = ids.pane();
    let document_id = ids.document();
    let workspace_id = ids.workspace();
    let mut panes = std::collections::HashMap::new();
    let mut pane = Pane::new(pane_id);
    pane.tabs.push(document_id);
    pane.active = Some(document_id);
    panes.insert(pane_id, pane);

    let mut documents = std::collections::HashMap::new();
    documents.insert(
        document_id,
        Document {
            id: document_id,
            path: Some(path.clone()),
            rope: ropey::Rope::from_str(text),
            dirty: false,
            eol: Eol::Lf,
            history: History::default(),
        },
    );

    let mut workspaces = WorkspaceManager::default();
    workspaces.push(Workspace::new(workspace_id, "test", pane_id));

    let mut state = AppState {
        should_quit: false,
        explorer: ExplorerState::new(dir.clone(), false).expect("explorer"),
        documents,
        panes,
        workspaces,
        focus: Focus::Pane(pane_id),
        mode: InputMode::Normal,
        keymap: KeyMap::default(),
        ui: ui::UiState::default(),
        config: AppConfig::default(),
        theme: crate::theme::Theme::default(),
        ids,
        warnings: Vec::new(),
        clipboard: String::new(),
        search: None,
        fullscreen: None,
        moving: false,
        shell: None,
        themes_dir: dir.join("themes"),
    };
    ui::compute_layout(&mut state, ratatui::layout::Rect::new(0, 0, 80, 24));
    (App::headless(state), dir)
}

fn text(app: &App) -> String {
    app.state
        .active_document()
        .map(|document| document.rope.to_string())
        .unwrap_or_default()
}

fn cursor(app: &App) -> usize {
    app.state
        .focused_pane()
        .map(|pane| pane.cursor)
        .unwrap_or(0)
}

#[test]
fn typing_and_undo_round_trips() {
    let (mut app, dir) = fixture("hello\n");
    app.dispatch(Command::MoveCursor(crate::command::Move::DocEnd))
        .expect("move");
    app.dispatch(Command::InsertText(" world".to_string()))
        .expect("insert");
    assert_eq!(text(&app), "hello\n world");
    assert!(app.state.active_document().expect("doc").dirty);

    app.dispatch(Command::Undo).expect("undo");
    assert_eq!(text(&app), "hello\n");
    app.dispatch(Command::Redo).expect("redo");
    assert_eq!(text(&app), "hello\n world");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_writes_the_buffer_and_clears_dirty() {
    let (mut app, dir) = fixture("one\n");
    app.dispatch(Command::InsertText("two".to_string()))
        .expect("insert");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::Save).expect("save");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "twoone\n");
    assert!(!app.state.active_document().expect("doc").dirty);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn selection_cut_and_paste_use_the_internal_clipboard() {
    let (mut app, dir) = fixture("abc\n");
    app.dispatch(Command::MoveCursor(crate::command::Move::DocEnd))
        .expect("move");
    app.dispatch(Command::SelectAll).expect("select");
    app.dispatch(Command::Cut).expect("cut");
    assert_eq!(text(&app), "");
    assert_eq!(app.state.clipboard, "abc\n");
    app.dispatch(Command::Paste).expect("paste");
    assert_eq!(text(&app), "abc\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn split_and_close_keep_the_layout_valid() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    assert_eq!(app.state.panes.len(), 2);
    assert_eq!(app.state.ui.pane_rects.len(), 2);
    for rect in app.state.ui.pane_rects.values() {
        assert!(rect.width >= MIN_COLS, "{rect:?}");
    }

    app.dispatch(Command::ClosePane).expect("close");
    assert_eq!(app.state.panes.len(), 1);
    assert!(app.state.workspaces.active().layout.leaves().len() == 1);

    assert!(app.dispatch(Command::ClosePane).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

const MIN_COLS: u16 = crate::workspace::MIN_PANE_COLS;

#[test]
fn workspaces_preserve_cursor_and_tabs() {
    let (mut app, dir) = fixture("alpha\nbeta\n");
    app.dispatch(Command::MoveCursor(crate::command::Move::Down))
        .expect("move");
    let remembered = cursor(&app);

    app.dispatch(Command::NewWorkspace).expect("workspace");
    assert_eq!(app.state.workspaces.workspaces.len(), 2);
    assert_ne!(app.state.workspaces.active, 0);

    app.dispatch(Command::SwitchWorkspace(0)).expect("switch");
    assert_eq!(app.state.workspaces.active, 0);
    assert_eq!(cursor(&app), remembered);
    assert_eq!(app.state.focused_pane().expect("pane").tabs.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn replace_all_changes_every_occurrence() {
    let (mut app, dir) = fixture("one two one\n");
    app.dispatch(Command::ReplacePrompt).expect("prompt");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.fields[0] = "one".to_string();
        prompt.fields[1] = "1".to_string();
    } else {
        panic!("expected replace prompt");
    }
    app.submit_prompt().expect("replace");
    assert_eq!(text(&app), "1 two 1\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn min_size_rejects_a_split_that_cannot_fit() {
    let (mut app, dir) = fixture("x\n");
    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 22, 10));
    for _ in 0..3 {
        let _ = app.dispatch(Command::SplitLeftRight);
    }
    for rect in app.state.ui.pane_rects.values() {
        assert!(rect.width >= crate::workspace::MIN_PANE_COLS || rect.width == 0);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn focus_direction_moves_between_panes() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    let first = app.state.workspaces.active().layout.leaves()[0];
    let second = app.state.workspaces.active().layout.leaves()[1];
    app.state.focus = Focus::Pane(second);
    app.dispatch(Command::FocusPane(crate::command::Direction::Left))
        .expect("focus");
    assert_eq!(app.state.focused_pane_id(), Some(first));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fullscreen_shows_only_the_focused_pane() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    assert_eq!(app.state.ui.pane_rects.len(), 2);

    let second = app.state.focused_pane_id().expect("focus");
    app.dispatch(Command::ToggleFullscreen).expect("fullscreen");
    assert_eq!(app.state.fullscreen, Some(second));
    assert_eq!(app.state.ui.pane_rects.len(), 1);
    assert_eq!(app.state.ui.pane_rects[&second], app.state.ui.panes);

    app.dispatch(Command::ToggleFullscreen).expect("restore");
    assert!(app.state.fullscreen.is_none());
    assert_eq!(app.state.ui.pane_rects.len(), 2);

    app.dispatch(Command::ToggleFullscreen)
        .expect("fullscreen again");
    app.dispatch(Command::ClosePane).expect("close");
    assert!(app.state.fullscreen.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn workspace_switch_drops_fullscreen() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::ToggleFullscreen).expect("fullscreen");
    app.dispatch(Command::NewWorkspace).expect("workspace");
    assert!(app.state.fullscreen.is_none());
    app.dispatch(Command::SwitchWorkspace(0)).expect("switch");
    assert!(app.state.fullscreen.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn move_pane_swaps_positions_and_keeps_focus() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    let second = app.state.focused_pane_id().expect("focus");
    let first = app.state.workspaces.active().layout.leaves()[0];
    let left_rect = app.state.ui.pane_rects[&first];
    let right_rect = app.state.ui.pane_rects[&second];

    app.dispatch(Command::MovePane(crate::command::Direction::Left))
        .expect("move");
    assert_eq!(app.state.focused_pane_id(), Some(second));
    assert_eq!(app.state.ui.pane_rects[&second], left_rect);
    assert_eq!(app.state.ui.pane_rects[&first], right_rect);
    assert_eq!(app.state.panes.len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn move_pane_without_neighbour_is_a_noop() {
    let (mut app, dir) = fixture("x\n");
    let before = app.state.ui.pane_rects.clone();
    app.dispatch(Command::MovePane(crate::command::Direction::Left))
        .expect("move");
    assert_eq!(app.state.ui.pane_rects, before);
    assert!(app.state.ui.message.contains("no pane"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn move_mode_routes_arrows_to_pane_moves() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    let second = app.state.focused_pane_id().expect("focus");

    app.dispatch(Command::ToggleMoveMode).expect("mode");
    assert!(app.state.moving);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.state.ui.pane_rects[&second].x, app.state.ui.panes.x);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.state.moving);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn opening_a_file_from_the_explorer_uses_the_last_pane() {
    let (mut app, dir) = fixture("first\n");
    let second_path = dir.join("second.txt");
    std::fs::write(&second_path, "second\n").expect("write");

    app.dispatch(Command::SplitLeftRight).expect("split");
    let second = app.state.focused_pane_id().expect("focus");
    let first = app.state.workspaces.active().layout.leaves()[0];

    // The explorer has focus, so the app must fall back to the remembered pane
    // instead of the first leaf.
    app.state.focus = Focus::Explorer;
    app.open_document(second_path.clone()).expect("open");

    assert_eq!(app.state.focused_pane_id(), Some(second));
    let active = app.state.panes[&second].active.expect("active");
    assert_eq!(
        app.state.documents[&active].path.as_deref(),
        Some(second_path.as_path())
    );
    assert_ne!(app.state.panes[&first].active, Some(active));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tabbed_layout_hides_siblings_and_restores() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    assert_eq!(app.state.ui.pane_rects.len(), 2);
    assert!(app.state.ui.tab_bars.is_empty());

    app.dispatch(Command::ToggleTabbed).expect("tabbed");
    assert_eq!(app.state.ui.pane_rects.len(), 1);
    assert_eq!(app.state.ui.tab_bars.len(), 1);
    assert_eq!(app.state.ui.tab_bars[0].tabs.len(), 2);
    assert!(app.state.ui.tab_bars[0].tabs[1].active);

    app.dispatch(Command::ToggleTabbed).expect("tiled");
    assert_eq!(app.state.ui.pane_rects.len(), 2);
    assert!(app.state.ui.tab_bars.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tabbed_needs_a_split_first() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::ToggleTabbed).expect("tabbed");
    assert!(app.state.ui.tab_bars.is_empty());
    assert!(app.state.ui.message.contains("split"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cycling_container_tabs_moves_focus() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    app.dispatch(Command::ToggleTabbed).expect("tabbed");
    let active = app.state.focused_pane_id().expect("focus");

    app.dispatch(Command::NextPaneTab).expect("next");
    let next = app.state.focused_pane_id().expect("focus");
    assert_ne!(active, next);
    assert!(app.state.ui.pane_rects.contains_key(&next));
    assert_eq!(app.state.ui.pane_rects.len(), 1);

    app.dispatch(Command::PrevPaneTab).expect("prev");
    assert_eq!(app.state.focused_pane_id(), Some(active));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clicking_a_container_tab_switches_to_it() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    app.dispatch(Command::ToggleTabbed).expect("tabbed");

    let bar = app.state.ui.tab_bars[0].clone();
    let target = bar.tabs[0].clone();
    let leaf = app.state.workspaces.active().layout.leaves()[0];

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: target.start + 1,
        row: bar.rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.state.focused_pane_id(), Some(leaf));
    assert!(app.state.ui.tab_bars[0].tabs[0].active);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn splitting_inside_a_tabbed_container_adds_a_tab() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    app.dispatch(Command::ToggleTabbed).expect("tabbed");
    assert_eq!(app.state.panes.len(), 2);

    app.dispatch(Command::SplitTopBottom).expect("split again");
    assert_eq!(app.state.panes.len(), 3);
    assert_eq!(app.state.ui.pane_rects.len(), 1);
    assert_eq!(app.state.ui.tab_bars[0].tabs.len(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

fn text_geometry(app: &App) -> crate::ui::pane::TextGeometry {
    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    let pane = app.state.panes.get(&pane_id).expect("pane");
    let document = app
        .state
        .documents
        .get(&pane.active.expect("document"))
        .expect("document");
    crate::ui::pane::text_geometry(&app.state, pane, document, rect).expect("geometry")
}

fn click(app: &mut App, column: u16, row: u16, modifiers: ratatui::crossterm::event::KeyModifiers) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers,
    });
}

fn wheel(app: &mut App, kind: ratatui::crossterm::event::MouseEventKind, column: u16, row: u16) {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent};

    app.handle_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

#[test]
fn clicking_text_places_the_cursor() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("alpha\nbeta\ngamma\n");
    let geometry = text_geometry(&app);
    let column = geometry.area.x + geometry.gutter + 2;
    let row = geometry.area.y + 1;

    click(&mut app, column, row, KeyModifiers::NONE);
    let expected = app
        .state
        .active_document()
        .map(|document| document.char_of_line(1) + 2)
        .expect("document");
    let pane = app.state.focused_pane().expect("pane");
    assert_eq!(pane.cursor, expected);
    assert!(pane.anchor.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shift_click_extends_the_selection() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("alpha\nbeta\n");
    let geometry = text_geometry(&app);
    let row = geometry.area.y;
    let first = geometry.area.x + geometry.gutter;
    let second = first + 2;

    click(&mut app, first, row, KeyModifiers::NONE);
    click(&mut app, second, row, KeyModifiers::SHIFT);
    let selection = app.state.focused_pane().expect("pane").selection();
    assert_eq!(selection, Some(0..2));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wheel_scrolls_the_pane_without_moving_the_cursor() {
    use ratatui::crossterm::event::MouseEventKind;

    let text = (0..60).map(|n| format!("line {n}\n")).collect::<String>();
    let (mut app, dir) = fixture(&text);
    let geometry = text_geometry(&app);
    let pane_id = app.state.focused_pane_id().expect("pane");
    let cursor = app.state.panes[&pane_id].cursor;
    let column = geometry.area.x + 1;
    let row = geometry.area.y + 1;

    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.panes[&pane_id].scroll_top, 3);
    assert_eq!(app.state.panes[&pane_id].cursor, cursor);

    wheel(&mut app, MouseEventKind::ScrollUp, column, row);
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn horizontal_wheel_scrolls_cells() {
    use ratatui::crossterm::event::MouseEventKind;

    let (mut app, dir) = fixture("abcdefghijklmnopqrstuvwxyz0123456789\n");
    let geometry = text_geometry(&app);
    let pane_id = app.state.focused_pane_id().expect("pane");
    let column = geometry.area.x + 1;
    let row = geometry.area.y;

    wheel(&mut app, MouseEventKind::ScrollRight, column, row);
    assert_eq!(app.state.panes[&pane_id].scroll_left, 4);
    wheel(&mut app, MouseEventKind::ScrollLeft, column, row);
    assert_eq!(app.state.panes[&pane_id].scroll_left, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wheel_over_the_explorer_moves_the_selection() {
    use ratatui::crossterm::event::MouseEventKind;

    let (mut app, dir) = fixture("x\n");
    for index in 0..6 {
        std::fs::write(dir.join(format!("f{index}.txt")), "x\n").expect("file");
    }
    app.state.explorer.refresh();
    let area = app.state.ui.explorer;
    let column = area.x + 2;
    let row = area.y + 2;

    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.explorer.selected, 3);
    assert_eq!(app.state.focus, Focus::Explorer);

    wheel(&mut app, MouseEventKind::ScrollUp, column, row);
    assert_eq!(app.state.explorer.selected, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clicking_an_explorer_row_selects_and_double_click_opens() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("x\n");
    std::fs::write(dir.join("target.txt"), "target\n").expect("file");
    app.state.explorer.refresh();
    let geometry = crate::ui::overlay::explorer_geometry(&app.state).expect("geometry");
    let index = app
        .state
        .explorer
        .rows
        .iter()
        .position(|row| row.name == "target.txt")
        .expect("row");
    let row = geometry.area.y + (index - geometry.scroll) as u16;
    let column = geometry.area.x + 2;

    click(&mut app, column, row, KeyModifiers::NONE);
    assert_eq!(app.state.explorer.selected, index);
    assert_eq!(app.state.focus, Focus::Explorer);

    click(&mut app, column, row, KeyModifiers::NONE);
    let name = app
        .state
        .active_document()
        .map(|document| document.display_name());
    assert_eq!(name.as_deref(), Some("target.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn splits_cannot_exceed_the_minimum_area() {
    let (mut app, dir) = fixture("x\n");
    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 80, 24));
    app.dispatch(Command::SplitLeftRight).expect("split");
    let layout = app.state.workspaces.active().layout.clone();
    assert!(matches!(
        layout,
        crate::workspace::LayoutNode::Split {
            axis: SplitAxis::LeftRight,
            ..
        }
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn theme_commands_cycle_and_select_files() {
    use crate::theme::Theme;
    use ratatui::style::Color;

    let (mut app, dir) = fixture("x\n");
    let themes = dir.join("themes");
    std::fs::create_dir_all(&themes).expect("themes");
    std::fs::write(
        themes.join("mine.toml"),
        "base = \"dark\"\nbackground = \"#010203\"\n",
    )
    .expect("write");
    app.state.themes_dir = themes;

    app.dispatch(Command::NextTheme).expect("next");
    assert_eq!(app.state.config.appearance.theme, "dark");
    assert_eq!(app.state.theme, Theme::preset("dark"));

    app.dispatch(Command::SelectTheme).expect("prompt");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.fields[0] = "mine".to_string();
        prompt.refilter();
    } else {
        panic!("expected theme prompt");
    }
    app.submit_prompt().expect("select");
    assert_eq!(app.state.config.appearance.theme, "mine");
    assert_eq!(app.state.theme.background, Color::Rgb(1, 2, 3));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn terminal_panel_reserves_rows_and_runs_a_shell() {
    use std::time::Duration;

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::ToggleTerminal).expect("toggle");
    assert!(app.state.shell.is_some());
    assert_eq!(app.state.focus, Focus::Shell);

    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 80, 24));
    assert_eq!(app.state.ui.shell.height, 12);
    assert!(app.state.ui.shell.y + app.state.ui.shell.height <= 23);
    assert!(app.state.ui.panes.height >= crate::workspace::MIN_PANE_ROWS);

    if let Some(shell) = app.state.shell.as_mut() {
        shell.write(b"printf 'panel-test\\n'\n");
    }
    let mut found = false;
    for _ in 0..150 {
        app.pump_shell();
        if let Some(shell) = app.state.shell.as_ref()
            && shell.screen().contents().contains("panel-test")
        {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(found, "shell output did not reach the screen");

    app.dispatch(Command::ToggleTerminal).expect("close");
    assert!(app.state.shell.is_none());
    assert!(matches!(app.state.focus, Focus::Pane(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shell_focus_routes_keys_away_from_the_document() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    let before = app
        .state
        .documents
        .values()
        .next()
        .map(|document| document.rope.to_string())
        .expect("document");
    app.dispatch(Command::ToggleTerminal).expect("toggle");
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let after = app
        .state
        .documents
        .values()
        .next()
        .map(|document| document.rope.to_string())
        .expect("document");
    assert_eq!(after, before);
    assert!(!app.state.documents.values().any(|document| document.dirty));
    std::fs::remove_dir_all(&dir).ok();
}
