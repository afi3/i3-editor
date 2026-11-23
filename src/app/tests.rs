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
    let mut document = Document {
        id: document_id,
        path: Some(path.clone()),
        rope: ropey::Rope::from_str(text),
        dirty: false,
        eol: Eol::Lf,
        history: History::default(),
        folded: Vec::new(),
        syntax: Default::default(),
        disk_modified: None,
        disk_len: 0,
    };
    document.record_disk_state();
    documents.insert(document_id, document);

    let mut workspaces = WorkspaceManager::default();
    workspaces.push(Workspace::new(workspace_id, "test", pane_id, dir.clone()));

    let mut state = AppState {
        should_quit: false,
        explorer: ExplorerState::new(dir.clone(), false).expect("explorer"),
        documents,
        panes,
        workspaces,
        focus: Focus::Pane(pane_id),
        mode: InputMode::Normal,
        keymap: KeyMap::preset(false),
        ui: ui::UiState::default(),
        config: AppConfig::default(),
        theme: crate::theme::Theme::default(),
        ids,
        warnings: Vec::new(),
        clipboard: String::new(),
        search: None,
        search_options: crate::search_options::Options::default(),
        project_search: None,
        lsp_diagnostics: std::collections::HashMap::new(),
        completions: None,
        hover: None,
        git_marks: std::collections::HashMap::new(),
        git_diff: None,
        help: None,
        hunks: None,
        plugin_status: Vec::new(),
        fullscreen: None,
        moving: false,
        terminals: Default::default(),
        themes_dir: dir.join("themes"),
        config_path: None,
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
fn find_in_files_opens_the_selected_match() {
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("one\n");
    std::fs::write(dir.join("other.txt"), "alpha\nneedle here\nalpha\n").expect("file");
    app.dispatch(Command::FindInFiles)
        .expect("open find in files");
    for ch in "needle".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.pump_project_search();
        if app.project_task.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.project_task.is_none(), "search did not finish");
    let search = app.state.project_search.as_ref().expect("results");
    assert_eq!(search.query, "needle");
    assert_eq!(search.results.len(), 1, "{:?}", search.results);
    assert!(search.results[0].path.ends_with("other.txt"));

    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("find-in-files prompt missing")
    };
    assert_eq!(prompt.candidates.len(), 1);
    app.submit_prompt().expect("jump");

    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    assert!(path.ends_with("other.txt"));
    let document = app.state.active_document().expect("doc");
    let cursor = app.state.focused_pane().expect("pane").cursor;
    assert_eq!(document.line_of_char(cursor), 1);
    assert_eq!(document.line_text(1).trim_start(), "needle here");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn move_lines_swaps_with_the_neighbour_and_undoes_once() {
    use crate::command::Move;

    let (mut app, dir) = fixture("one\ntwo\nthree\n");
    app.dispatch(Command::MoveCursor(Move::Down))
        .expect("line 1");
    app.dispatch(Command::MoveLinesUp).expect("move up");
    assert_eq!(text(&app), "two\none\nthree\n");
    assert_eq!(
        app.state
            .active_document()
            .expect("doc")
            .line_of_char(app.state.focused_pane().expect("pane").cursor),
        0
    );
    app.dispatch(Command::Undo).expect("undo");
    assert_eq!(text(&app), "one\ntwo\nthree\n");

    app.dispatch(Command::MoveLinesDown).expect("move down");
    assert_eq!(text(&app), "one\nthree\ntwo\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_line_selection_moves_as_a_block() {
    use crate::command::Move;

    let (mut app, dir) = fixture("one\ntwo\nthree\n");
    app.dispatch(Command::ExtendSelection(Move::Down))
        .expect("down");
    app.dispatch(Command::ExtendSelection(Move::Down))
        .expect("down");
    app.dispatch(Command::MoveLinesDown).expect("move down");
    assert_eq!(text(&app), "three\none\ntwo\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn moving_past_the_first_line_is_refused() {
    let (mut app, dir) = fixture("one\ntwo\n");
    app.dispatch(Command::MoveLinesUp).expect("move up");
    assert_eq!(text(&app), "one\ntwo\n");
    assert!(
        app.state.ui.message.contains("first line"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn delete_line_removes_the_current_line() {
    use crate::command::Move;

    let (mut app, dir) = fixture("one\ntwo\nthree\n");
    app.dispatch(Command::MoveCursor(Move::Down))
        .expect("line 1");
    app.dispatch(Command::DeleteLine).expect("delete");
    assert_eq!(text(&app), "one\nthree\n");
    app.dispatch(Command::Undo).expect("undo");
    assert_eq!(text(&app), "one\ntwo\nthree\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn delete_last_line_without_newline_keeps_the_previous_line() {
    use crate::command::Move;

    let (mut app, dir) = fixture("one\ntwo");
    app.dispatch(Command::MoveCursor(Move::DocEnd))
        .expect("end");
    app.dispatch(Command::DeleteLine).expect("delete");
    assert_eq!(text(&app), "one\n");
    std::fs::remove_dir_all(&dir).ok();
}

fn set_document_path(app: &mut App, name: &str) {
    let id = app.state.active_document_id().expect("document");
    let parent = app
        .state
        .documents
        .get(&id)
        .and_then(|document| document.path.as_ref())
        .and_then(|path| path.parent())
        .expect("parent")
        .to_path_buf();
    app.state.documents.get_mut(&id).expect("doc").path = Some(parent.join(name));
}

#[test]
fn toggle_comment_adds_and_removes_line_prefixes() {
    let (mut app, dir) = fixture("let a = 1;\n");
    set_document_path(&mut app, "note.rs");
    app.dispatch(Command::ToggleComment).expect("comment");
    assert_eq!(text(&app), "// let a = 1;\n");
    app.dispatch(Command::ToggleComment).expect("uncomment");
    assert_eq!(text(&app), "let a = 1;\n");
    app.dispatch(Command::ToggleComment).expect("comment");
    app.dispatch(Command::Undo).expect("undo");
    assert_eq!(text(&app), "let a = 1;\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn toggle_comment_uses_the_block_form_for_markdown() {
    let (mut app, dir) = fixture("hello\n");
    set_document_path(&mut app, "note.md");
    app.dispatch(Command::ToggleComment).expect("comment");
    assert_eq!(text(&app), "<!-- hello -->\n");
    app.dispatch(Command::ToggleComment).expect("uncomment");
    assert_eq!(text(&app), "hello\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn toggle_comment_reports_languages_without_comments() {
    let (mut app, dir) = fixture("{\"a\": 1}\n");
    set_document_path(&mut app, "note.json");
    app.dispatch(Command::ToggleComment).expect("comment");
    assert_eq!(text(&app), "{\"a\": 1}\n");
    assert!(
        app.state.ui.message.contains("no comment syntax"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn typing_brackets_pairs_them_and_skips_the_closer() {
    use ratatui::crossterm::event::KeyCode;

    let (mut app, dir) = fixture("");
    press(&mut app, KeyCode::Char('('));
    assert_eq!(text(&app), "()");
    assert_eq!(app.state.focused_pane().expect("pane").cursor, 1);

    press(&mut app, KeyCode::Char(')'));
    assert_eq!(text(&app), "()");
    assert_eq!(app.state.focused_pane().expect("pane").cursor, 2);

    press(&mut app, KeyCode::Char('"'));
    assert_eq!(text(&app), "()\"\"");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markdown_backticks_do_not_auto_pair() {
    use ratatui::crossterm::event::KeyCode;

    let (mut app, dir) = fixture("");
    set_document_path(&mut app, "note.md");
    press(&mut app, KeyCode::Char('`'));
    assert_eq!(text(&app), "`");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dragging_inside_a_pane_selects_text() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let (mut app, dir) = fixture("hello world\n");
    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    let geometry = {
        let pane = app.state.panes.get(&pane_id).expect("pane");
        let document = app.state.active_document().expect("document");
        crate::ui::pane::text_geometry(&app.state, pane, document, rect).expect("geometry")
    };
    let y = geometry.area.y;
    let x = geometry.area.x + geometry.gutter;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x + 1,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: x + 6,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    let pane = app.state.panes.get(&pane_id).expect("pane");
    assert_eq!(pane.selection(), Some(1..6));
    assert_eq!(app.selecting, Some(pane_id));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x + 6,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selecting, None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wheel_moves_the_prompt_selection() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::Palette).expect("palette");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("palette missing")
    };
    assert!(prompt.filtered.len() > 3);
    wheel(
        &mut app,
        ratatui::crossterm::event::MouseEventKind::ScrollDown,
        1,
        1,
    );
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("palette closed")
    };
    assert_eq!(prompt.selected, 3);
    wheel(
        &mut app,
        ratatui::crossterm::event::MouseEventKind::ScrollUp,
        1,
        1,
    );
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("palette closed")
    };
    assert_eq!(prompt.selected, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clicking_a_workspace_tab_switches_workspaces() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::NewWorkspace).expect("new workspace");
    assert_eq!(app.state.workspaces.active, 1);
    crate::ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 80, 24));
    let hit = app.state.ui.workspace_hits[0].clone();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: hit.start + 1,
        row: app.state.ui.workspace_bar.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.state.workspaces.active, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_file_changes_reload_settings() {
    use std::time::{Duration, Instant};

    let (mut app, dir) = fixture("x\n");
    let config = dir.join("config.toml");
    std::fs::write(&config, "[editor]\ntab_size = 2\n").expect("config");
    app.state.config_path = Some(config.clone());
    app.enhanced = true;
    let workspace = app.state.workspaces.active().id;
    app.watchers = vec![(
        workspace,
        crate::watch::Watcher::new(&dir, Some(&config)).expect("watch"),
    )];
    std::fs::write(&config, "[editor]\ntab_size = 6\n").expect("rewrite");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && app.state.config.editor.tab_size != 6 {
        app.pump_watcher();
        if app.state.config.editor.tab_size != 6 {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert_eq!(app.state.config.editor.tab_size, 6);
    assert_eq!(app.state.ui.message, "config reloaded");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn paste_into_a_prompt_edits_the_field_not_the_document() {
    let (mut app, dir) = fixture("one\n");
    app.dispatch(Command::FindPrompt).expect("find");
    app.handle_paste("needle".to_string()).expect("paste");

    assert_eq!(text(&app), "one\n", "paste must not reach the document");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("prompt closed by paste")
    };
    assert_eq!(prompt.input(), "needle");
    assert_eq!(
        app.state
            .search
            .as_ref()
            .map(|search| search.query.as_str()),
        Some("needle"),
        "find preview must follow the pasted query"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn paste_goes_to_the_focused_shell() {
    use crate::shell::ShellPanel;
    use crate::state::Focus;
    use std::time::Duration;

    let (mut app, dir) = fixture("one\n");
    let panel = ShellPanel::spawn_shell(&dir, 10, 60, "/bin/sh", 2000).expect("shell");
    app.state.terminals.push(panel);
    app.state.focus = Focus::Shell;
    app.handle_paste("printf 'paste-ok\\n'\n".to_string())
        .expect("paste");

    let mut visible = String::new();
    for _ in 0..100 {
        app.pump_shell();
        visible = app
            .state
            .terminals
            .active()
            .expect("shell")
            .screen()
            .contents();
        if visible.contains("paste-ok") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(visible.contains("paste-ok"), "screen: {visible:?}");
    assert_eq!(
        app.state
            .documents
            .values()
            .next()
            .expect("document")
            .rope
            .to_string(),
        "one\n",
        "shell paste must not touch the document"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mouse_is_ignored_while_a_prompt_is_open() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let (mut app, dir) = fixture("one\ntwo\nthree\n");
    app.dispatch(Command::FindPrompt).expect("find");
    let pane_id = app.state.focused_pane_id().expect("pane");
    let cursor = app.state.panes[&pane_id].cursor;
    let area = app.state.ui.pane_rects[&pane_id];

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: area.x + 3,
        row: area.y + 2,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.state.mode.is_prompt(), "prompt must stay open");
    assert_eq!(app.state.focus, Focus::Pane(pane_id));
    assert_eq!(app.state.panes[&pane_id].cursor, cursor);
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
fn shell_mouse_reports_reach_the_program() {
    use crate::shell::ShellPanel;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let (mut app, dir) = fixture("note\n");
    app.state.ui.shell = ratatui::layout::Rect::new(0, 10, 60, 7);
    let mut panel = ShellPanel::spawn_shell(&dir, 5, 58, "/bin/sh", 2000).expect("shell");
    panel.write(b"printf '\\033[?1000h\\033[?1006h'; cat -v\n");
    for _ in 0..100 {
        panel.pump();
        if panel.reports_mouse() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(panel.reports_mouse(), "shell enabled mouse reporting");
    app.state.terminals.push(panel);

    // Body starts at (1, 11); (6, 12) is one-based (6, 2).
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 6,
        row: 12,
        modifiers: KeyModifiers::NONE,
    });

    let mut screen = String::new();
    for _ in 0..100 {
        app.pump_shell();
        screen = app
            .state
            .terminals
            .active()
            .expect("shell")
            .screen()
            .contents();
        if screen.contains("^[[<0;6;2M") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(screen.contains("^[[<0;6;2M"), "screen: {screen:?}");
    assert_eq!(app.state.focus, Focus::Shell);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clicking_a_document_tab_activates_it() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("one\n");
    let other = dir.join("other.txt");
    std::fs::write(&other, "two\n").expect("other");
    app.open_document(other).expect("open other");
    assert_eq!(
        app.state
            .active_document()
            .map(|document| document.display_name()),
        Some("other.txt".to_string())
    );

    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    click(&mut app, rect.x + 1, rect.y + 1, KeyModifiers::NONE);
    assert_eq!(
        app.state
            .active_document()
            .map(|document| document.display_name()),
        Some("note.txt".to_string())
    );
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
fn rename_terminal_updates_the_active_tab_and_requires_a_panel() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::ToggleTerminal).expect("open");
    app.dispatch(Command::TerminalRename).expect("prompt");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("rename prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::TerminalRename);
    prompt.fields[0] = "build".to_string();
    app.submit_prompt().expect("rename");

    assert_eq!(
        app.state.terminals.active().expect("shell").title(),
        "build"
    );
    assert!(app.state.ui.message.contains("terminal renamed"));

    app.dispatch(Command::TerminalClose).expect("close");
    app.dispatch(Command::TerminalRename)
        .expect("rename without terminal");
    assert!(matches!(app.state.mode, InputMode::Normal));
    assert!(app.state.ui.message.contains("no terminal open"));
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
    assert!(!app.state.terminals.is_empty());
    assert_eq!(app.state.focus, Focus::Shell);

    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 80, 24));
    assert_eq!(app.state.ui.shell.height, 12);
    assert!(app.state.ui.shell.y + app.state.ui.shell.height <= 23);
    assert!(app.state.ui.panes.height >= crate::workspace::MIN_PANE_ROWS);

    if let Some(shell) = app.state.terminals.active_mut() {
        shell.write(b"printf 'panel-test\\n'\n");
    }
    let mut found = false;
    for _ in 0..150 {
        app.pump_shell();
        if let Some(shell) = app.state.terminals.active()
            && shell.screen().contents().contains("panel-test")
        {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(found, "shell output did not reach the screen");

    app.dispatch(Command::ToggleTerminal).expect("close");
    assert!(app.state.terminals.is_empty());
    assert!(matches!(app.state.focus, Focus::Pane(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn terminal_tabs_open_cycle_and_close() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::ToggleTerminal).expect("open");
    assert_eq!(app.state.terminals.len(), 1);

    app.dispatch(Command::TerminalNew).expect("new tab");
    assert_eq!(app.state.terminals.len(), 2);
    assert_eq!(app.state.terminals.active_index(), 1);

    // Ctrl+PageUp/PageDown switch terminal tabs while the shell has focus.
    app.dispatch(Command::PrevPaneTab).expect("previous tab");
    assert_eq!(app.state.terminals.active_index(), 0);
    app.dispatch(Command::NextPaneTab).expect("next tab");
    assert_eq!(app.state.terminals.active_index(), 1);

    app.dispatch(Command::TerminalClose).expect("close tab");
    assert_eq!(app.state.terminals.len(), 1);
    assert_eq!(app.state.focus, Focus::Shell);

    app.dispatch(Command::TerminalClose)
        .expect("close last tab");
    assert!(app.state.terminals.is_empty());
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

#[test]
fn idle_timeout_is_one_second_without_shell_or_lsp() {
    use std::time::Duration;

    let (app, dir) = fixture("x\n");
    assert_eq!(app.frame_timeout(), Duration::from_secs(1));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quit_with_clean_documents_asks_first() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::Quit).expect("quit");
    assert!(!app.state.should_quit, "clean quit must still confirm");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("quit confirmation missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::QuitConfirm);
    assert_eq!(prompt.selected_candidate(), Some("Quit"));
    app.submit_prompt().expect("confirm quit");
    assert!(app.state.should_quit);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quit_confirmation_can_be_cancelled() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::Quit).expect("quit");
    assert!(!app.state.should_quit);
    app.cancel_prompt();
    assert!(!app.state.should_quit);
    assert!(matches!(app.state.mode, InputMode::Normal));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quit_with_modified_documents_asks_first() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    app.dispatch(Command::Quit).expect("quit");
    assert!(!app.state.should_quit, "must not quit with unsaved changes");
    assert!(matches!(app.state.mode, InputMode::Prompt(_)));

    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.selected = 1; // "Discard all and quit"
    }
    app.submit_prompt().expect("discard");
    assert!(app.state.should_quit);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quit_can_save_every_document_first() {
    let (mut app, dir) = fixture("x\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    app.dispatch(Command::Quit).expect("quit");

    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        assert_eq!(prompt.selected_candidate(), Some("Save all and quit"));
    }
    app.submit_prompt().expect("save and quit");
    assert!(app.state.should_quit);
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "yx\n");
    assert!(!app.state.documents.values().any(|document| document.dirty));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn new_prompt_creates_a_file_on_disk() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::NewPrompt).expect("new file");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("new-file prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::NewFile);
    prompt.fields[0] = "created.txt".to_string();
    app.submit_prompt().expect("create");

    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    assert!(path.ends_with("created.txt"), "{path:?}");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn duplicate_file_copies_with_unique_names_and_refuses_dirty_buffers() {
    let (mut app, dir) = fixture("note\n");
    app.dispatch(Command::DuplicateFile).expect("duplicate");
    assert_eq!(
        std::fs::read_to_string(dir.join("note copy.txt")).expect("first copy"),
        "note\n"
    );
    app.dispatch(Command::DuplicateFile)
        .expect("duplicate again");
    assert_eq!(
        std::fs::read_to_string(dir.join("note copy 2.txt")).expect("second copy"),
        "note\n"
    );

    app.dispatch(Command::InsertText("x".to_string()))
        .expect("dirty buffer");
    app.dispatch(Command::DuplicateFile)
        .expect("refuse duplicate");
    assert!(!dir.join("note copy 3.txt").exists());
    assert!(app.state.ui.message.contains("save the file"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn copy_path_targets_the_active_document_or_explorer_selection() {
    let (mut app, dir) = fixture("note\n");
    let active = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::CopyPath).expect("copy active path");
    assert_eq!(app.state.clipboard, active.to_string_lossy());

    std::fs::write(dir.join("other.txt"), "other\n").expect("file");
    app.state.explorer.refresh();
    let index = app
        .state
        .explorer
        .rows
        .iter()
        .position(|row| row.name == "other.txt")
        .expect("row");
    app.state.explorer.selected = index;
    app.state.focus = Focus::Explorer;
    app.dispatch(Command::CopyPath).expect("copy selected path");
    assert_eq!(
        app.state.clipboard,
        dir.join("other.txt")
            .canonicalize()
            .expect("canonical")
            .to_string_lossy()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reveal_in_terminal_starts_a_shell_in_the_parent_directory() {
    let (mut app, dir) = fixture("note\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    let expected = path.parent().expect("parent").to_path_buf();

    app.dispatch(Command::RevealInTerminal).expect("reveal");
    assert_eq!(app.state.terminals.len(), 1);
    assert_eq!(app.state.focus, Focus::Shell);
    assert_eq!(
        app.state.terminals.active().expect("shell").cwd(),
        expected.as_path()
    );
    assert!(
        app.state
            .ui
            .message
            .contains(&expected.display().to_string()),
        "{}",
        app.state.ui.message
    );
    app.state.terminals = crate::shell::Terminals::default();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rename_workspace_updates_the_active_name() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::RenameWorkspace).expect("prompt");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("rename prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::RenameWorkspace);
    assert_eq!(prompt.fields[0], "test");
    prompt.fields[0] = "main".to_string();
    app.submit_prompt().expect("rename");

    assert_eq!(app.state.workspaces.active().name, "main");
    assert!(app.state.ui.message.contains("workspace renamed"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cli_targets_open_multiple_files_as_tabs_and_jump_to_a_line() {
    let (mut app, dir) = fixture("one\n");
    let second = dir.join("second.txt");
    let third = dir.join("third.txt");
    std::fs::write(&second, "a\nb\nc\n").expect("second");
    std::fs::write(&third, "x\n").expect("third");

    app.open_targets(vec![super::OpenTarget {
        path: second.clone(),
        line: Some(3),
    }])
    .expect("single target");
    let document = app.state.active_document().expect("second");
    assert_eq!(document.path.as_deref(), Some(second.as_path()));
    let cursor = app.state.focused_pane().expect("pane").cursor;
    assert_eq!(document.line_of_char(cursor), 2);

    app.open_targets(vec![
        super::OpenTarget {
            path: third.clone(),
            line: None,
        },
        super::OpenTarget {
            path: second.clone(),
            line: None,
        },
    ])
    .expect("multiple targets");
    assert_eq!(app.state.documents.len(), 3);
    assert_eq!(app.state.focused_pane().expect("pane").tabs.len(), 3);
    assert_eq!(
        app.state
            .active_document()
            .and_then(|document| document.path.clone()),
        Some(second)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_prompt_opens_a_typed_path() {
    let (mut app, dir) = fixture("x\n");
    std::fs::write(dir.join("other.txt"), "other\n").expect("file");
    app.dispatch(Command::OpenPrompt).expect("open");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("open prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::OpenPath);
    prompt.fields[0] = "other.txt".to_string();
    app.submit_prompt().expect("open path");

    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    assert!(path.ends_with("other.txt"), "{path:?}");
    assert_eq!(text(&app), "other\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn oversized_copy_reports_the_internal_clipboard() {
    let big = "a".repeat(50_000);
    let (mut app, dir) = fixture(&big);
    app.dispatch(Command::SelectAll).expect("select");
    app.dispatch(Command::Copy).expect("copy");
    assert_eq!(app.state.clipboard, big);
    assert!(
        app.state.ui.message.contains("internal clipboard only"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn closing_a_pane_with_unsaved_changes_is_refused() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    assert_eq!(app.state.panes.len(), 2);

    app.dispatch(Command::ClosePane).expect("close");
    assert_eq!(app.state.panes.len(), 2, "pane must stay open");
    assert!(app.state.ui.message.contains("unsaved"));

    app.dispatch(Command::Save).expect("save");
    app.dispatch(Command::ClosePane).expect("close");
    assert_eq!(app.state.panes.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_back_to_the_saved_revision_clears_modified() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    assert!(app.state.active_document().expect("doc").dirty);

    app.dispatch(Command::Save).expect("save");
    assert!(!app.state.active_document().expect("doc").dirty);

    app.dispatch(Command::Undo).expect("undo");
    assert!(app.state.active_document().expect("doc").dirty);

    app.dispatch(Command::Redo).expect("redo");
    assert!(!app.state.active_document().expect("doc").dirty);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn theme_choice_is_written_back_to_the_config_file() {
    let (mut app, dir) = fixture("x\n");
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, "[editor]\ntab_size = 2\n").expect("write");
    app.state.config_path = Some(config_path.clone());

    app.dispatch(Command::NextTheme).expect("next");
    let text = std::fs::read_to_string(&config_path).expect("read");
    assert!(text.contains("theme = \"dark\""), "{text}");
    assert!(text.contains("tab_size = 2"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_q_key_asks_before_quitting() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
    assert!(!app.state.should_quit);
    assert!(matches!(app.state.mode, InputMode::Prompt(_)));
    std::fs::remove_dir_all(&dir).ok();
}

fn press(app: &mut App, code: ratatui::crossterm::event::KeyCode) {
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        code,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
}

fn type_query(app: &mut App, query: &str) {
    for ch in query.chars() {
        press(app, ratatui::crossterm::event::KeyCode::Char(ch));
    }
}

#[test]
fn find_previews_every_match_while_typing() {
    let (mut app, dir) = fixture("alpha\nbeta alpha\n");
    app.dispatch(Command::FindPrompt).expect("open find");
    type_query(&mut app, "alpha");

    let search = app.state.search.as_ref().expect("live search");
    assert_eq!(search.matches, vec![0..5, 11..16]);
    assert_eq!(search.index, 0);
    assert_eq!(cursor(&app), 0);
    assert_eq!(app.state.ui.message, "alpha: match 1/2");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn regex_toggle_changes_find_matches() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("alpha beta\n");
    app.dispatch(Command::FindPrompt).expect("find");
    type_query(&mut app, "a.*a");
    let search = app.state.search.as_ref().expect("search");
    assert!(search.matches.is_empty(), "literal mode should not match");

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT));
    assert!(app.state.search_options.regex);
    let search = app.state.search.as_ref().expect("search");
    assert_eq!(search.matches, vec![0..10]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn case_toggle_narrows_find_matches() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("Alpha alpha\n");
    app.dispatch(Command::FindPrompt).expect("find");
    type_query(&mut app, "alpha");
    assert_eq!(app.state.search.as_ref().expect("search").matches.len(), 2);

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT));
    assert!(app.state.search_options.case_sensitive);
    let search = app.state.search.as_ref().expect("search");
    assert_eq!(search.matches, vec![6..11]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn regex_replace_expands_capture_groups() {
    let (mut app, dir) = fixture("one two\n");
    app.dispatch(Command::ReplacePrompt).expect("replace");
    app.state.search_options.regex = true;
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("replace prompt")
    };
    prompt.fields[0] = r"(\w+)".to_string();
    prompt.fields[1] = "$1!".to_string();
    app.submit_prompt().expect("replace");
    assert_eq!(text(&app), "one! two!\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn find_arrow_picks_a_match_and_enter_keeps_it() {
    let (mut app, dir) = fixture("alpha\nbeta alpha\n");
    app.dispatch(Command::FindPrompt).expect("open find");
    type_query(&mut app, "alpha");
    press(&mut app, ratatui::crossterm::event::KeyCode::Down);

    let search = app.state.search.as_ref().expect("live search");
    assert_eq!(search.index, 1);
    assert_eq!(cursor(&app), 11);
    assert_eq!(
        app.state.focused_pane().expect("pane").selection(),
        Some(11..16)
    );

    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    assert!(!app.state.mode.is_prompt());
    let search = app.state.search.as_ref().expect("kept search");
    assert_eq!(search.index, 1);
    assert_eq!(search.matches.len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn find_without_matches_keeps_an_empty_match_list() {
    let (mut app, dir) = fixture("alpha\n");
    app.dispatch(Command::FindPrompt).expect("open find");
    type_query(&mut app, "zzz");

    let search = app.state.search.as_ref().expect("search state");
    assert!(search.matches.is_empty());
    assert_eq!(app.state.ui.message, "no match for zzz");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn next_match_advances_and_wraps() {
    let (mut app, dir) = fixture("alpha\nbeta alpha\n");
    app.dispatch(Command::FindPrompt).expect("open find");
    type_query(&mut app, "alpha");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);

    app.dispatch(Command::NextMatch).expect("next");
    assert_eq!(cursor(&app), 11);
    app.dispatch(Command::NextMatch).expect("wrap");
    assert_eq!(cursor(&app), 0);
    std::fs::remove_dir_all(&dir).ok();
}
#[test]
fn quick_open_uses_fuzzy_ranking() {
    let (mut app, dir) = fixture("note\n");
    std::fs::create_dir_all(dir.join("src")).expect("src dir");
    std::fs::write(dir.join("src").join("main.rs"), "fn main() {}\n").expect("file");
    std::fs::create_dir_all(dir.join("docs")).expect("docs dir");
    std::fs::write(dir.join("docs").join("domain_main.md"), "# notes\n").expect("doc");

    app.dispatch(Command::QuickOpen).expect("quick open");
    type_query(&mut app, "sm");

    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("quick open prompt missing")
    };
    assert_eq!(prompt.selected_candidate(), Some("src/main.rs"));

    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("opened document");
    assert!(path.ends_with("src/main.rs"), "{}", path.display());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quick_open_lists_dotfiles_when_hidden_is_shown() {
    let (mut app, dir) = fixture("note\n");
    std::fs::write(dir.join(".gitignore"), ".env\n").expect("gitignore");
    std::fs::write(dir.join(".env"), "SECRET=1\n").expect("dotfile");

    app.dispatch(Command::QuickOpen).expect("quick open");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("quick open prompt missing")
    };
    assert!(!prompt.candidates.iter().any(|name| name == ".env"));

    press(&mut app, ratatui::crossterm::event::KeyCode::Esc);
    app.dispatch(Command::ExplorerToggleHidden).expect("toggle");
    app.dispatch(Command::QuickOpen).expect("quick open");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("quick open prompt missing")
    };
    assert!(
        prompt.candidates.iter().any(|name| name == ".env"),
        "{:?}",
        prompt.candidates
    );

    press(&mut app, ratatui::crossterm::event::KeyCode::Esc);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_inserts_spaces_up_to_the_next_stop() {
    let (mut app, dir) = fixture("x\n");
    app.state.config.editor.tab_size = 4;
    app.dispatch(Command::InsertTab).expect("tab");
    assert_eq!(text(&app), "    x\n");
    assert_eq!(cursor(&app), 4);

    let (mut app, dir_two) = fixture("x\n");
    app.state.config.editor.tab_size = 4;
    app.dispatch(Command::InsertText("abc".to_string()))
        .expect("type");
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    assert_eq!(
        text(&app),
        "abc x\n",
        "one space reaches the stop at column 4"
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&dir_two).ok();
}

#[test]
fn tab_is_literal_when_insert_spaces_is_off() {
    let (mut app, dir) = fixture("x\n");
    app.state.config.editor.insert_spaces = false;
    app.dispatch(Command::InsertTab).expect("tab");
    assert_eq!(text(&app), "\tx\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_as_existing_path_asks_before_overwriting() {
    let (mut app, dir) = fixture("one\n");
    let other = dir.join("other.txt");
    std::fs::write(&other, "keep\n").expect("seed");

    app.dispatch(Command::SavePrompt).expect("save as");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("save-as prompt missing")
    };
    prompt.fields[0] = other.to_string_lossy().into_owned();
    app.submit_prompt().expect("submit");

    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("overwrite confirmation missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::Overwrite);
    assert_eq!(prompt.target.as_deref(), Some(other.as_path()));
    assert_eq!(std::fs::read_to_string(&other).expect("read"), "keep\n");

    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    assert!(!app.state.mode.is_prompt());
    assert_eq!(std::fs::read_to_string(&other).expect("read"), "one\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_as_new_path_writes_without_confirmation() {
    let (mut app, dir) = fixture("one\n");
    let new = dir.join("new.txt");

    app.dispatch(Command::SavePrompt).expect("save as");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("save-as prompt missing")
    };
    prompt.fields[0] = new.to_string_lossy().into_owned();
    app.submit_prompt().expect("submit");

    assert!(!app.state.mode.is_prompt());
    assert_eq!(std::fs::read_to_string(&new).expect("read"), "one\n");
    assert_eq!(
        app.state
            .active_document()
            .and_then(|document| document.path.clone()),
        Some(new)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_visibility_is_applied_at_startup() {
    use crate::terminal::Setup;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;

    let backend = CrosstermBackend::new(std::io::stdout());
    let terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        },
    )
    .expect("terminal");

    let mut config = AppConfig::default();
    config.explorer.visible = false;
    let dir = std::env::temp_dir().join(format!("i3-config-vis-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let app = App::new(
        Setup {
            terminal,
            keyboard_enhancement: false,
        },
        config,
        Vec::new(),
        vec![super::OpenTarget {
            path: dir.clone(),
            line: None,
        }],
    )
    .expect("app");
    assert!(!app.state.explorer.visible);
    assert!(
        app.state
            .ui
            .message
            .contains("keyboard enhancement unavailable"),
        "{}",
        app.state.ui.message
    );
    assert!(
        !app.state
            .keymap
            .global
            .values()
            .any(|command| matches!(command, Command::SwitchWorkspace(0))),
        "ctrl+1 must be dropped without enhancement"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_workspace_cycling_wraps() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::NewWorkspace).expect("new workspace");
    assert_eq!(app.state.workspaces.active, 1);
    app.dispatch(Command::NextWorkspace).expect("next");
    assert_eq!(app.state.workspaces.active, 0);
    app.dispatch(Command::PrevWorkspace).expect("previous");
    assert_eq!(app.state.workspaces.active, 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn messages_lists_every_startup_warning() {
    let (mut app, dir) = fixture("x\n");
    app.state.warnings = vec!["first warning".to_string(), "second warning".to_string()];
    app.dispatch(Command::Messages).expect("messages");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("messages prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::Messages);
    assert_eq!(prompt.candidates, app.state.warnings);
    app.submit_prompt().expect("close");
    assert!(!app.state.mode.is_prompt());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn startup_message_points_at_the_remaining_warnings() {
    assert_eq!(super::startup_message(&[]), "");
    assert_eq!(super::startup_message(&["only".to_string()]), "only");
    assert_eq!(
        super::startup_message(&["first".to_string(), "second".to_string()]),
        "first (+1 more; palette: messages)"
    );
}

#[test]
fn palette_lists_and_switches_workspaces() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::NewWorkspace).expect("new workspace");
    assert_eq!(app.state.workspaces.active, 1);

    app.dispatch(Command::Palette).expect("palette");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("palette prompt missing")
    };
    assert!(
        prompt
            .candidates
            .iter()
            .any(|name| name == "switch_workspace_1")
    );
    assert!(
        prompt
            .candidates
            .iter()
            .any(|name| name == "switch_workspace_2")
    );
    assert!(
        prompt
            .candidates
            .iter()
            .any(|name| name == "explorer_refresh")
    );
    prompt.select_candidate("switch_workspace_1");
    app.submit_prompt().expect("switch");
    assert_eq!(app.state.workspaces.active, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_c_cancels_an_active_prompt() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::FindPrompt).expect("find");
    assert!(app.state.mode.is_prompt());
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app.state.mode.is_prompt(), "ctrl+c must close the prompt");
    assert_eq!(text(&app), "x\n", "ctrl+c must not edit the document");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn debug_log_writes_the_resolved_keymap() {
    let dir = std::env::temp_dir().join(format!("i3-keymap-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let config = dir.join("config.toml");
    super::log_keymap(Some(&config), &crate::input::KeyMap::preset(false), false);
    let log = std::fs::read_to_string(dir.join("keymap.log")).expect("log");
    assert!(log.contains("keyboard_enhancement: false"), "{log}");
    assert!(log.contains("Ctrl+Q -> quit"), "{log}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_after_an_external_change_asks_and_can_reload() {
    let (mut app, dir) = fixture("one\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::InsertText("two".to_string()))
        .expect("type");
    std::fs::write(&path, "outside change\n").expect("external write");

    app.dispatch(Command::Save).expect("save");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("external-change prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::ExternalChange);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "outside change\n",
        "save must not run before the choice"
    );

    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.move_selection(1);
    }
    app.submit_prompt().expect("reload");
    assert!(!app.state.mode.is_prompt());
    assert_eq!(text(&app), "outside change\n");
    assert!(!app.state.active_document().expect("doc").dirty);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn overwrite_after_an_external_change_writes_the_buffer() {
    let (mut app, dir) = fixture("one\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::InsertText("two".to_string()))
        .expect("type");
    std::fs::write(&path, "outside\n").expect("external write");

    app.dispatch(Command::Save).expect("save");
    assert!(app.state.mode.is_prompt(), "prompt expected");
    app.submit_prompt().expect("overwrite");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "twoone\n");
    assert!(!app.state.active_document().expect("doc").dirty);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn watcher_reloads_a_clean_buffer_and_refreshes_the_explorer() {
    use std::time::{Duration, Instant};

    let (mut app, dir) = fixture("one\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    let workspace = app.state.workspaces.active().id;
    app.watchers = vec![(
        workspace,
        crate::watch::Watcher::new(&dir, None).expect("watch"),
    )];

    // Warm the quick-open cache so the assertions below prove invalidation.
    app.open_prompt(crate::input::PromptKind::QuickOpen);
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("quick-open prompt missing")
    };
    assert!(
        !prompt
            .candidates
            .iter()
            .any(|candidate| candidate == "new.txt")
    );
    app.state.mode = InputMode::Normal;

    std::fs::write(&path, "two\n").expect("external write");
    std::fs::write(dir.join("new.txt"), "x\n").expect("new file");

    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut reloaded, mut listed) = (false, false);
    while Instant::now() < deadline && !(reloaded && listed) {
        app.pump_watcher();
        reloaded = text(&app) == "two\n";
        listed = app
            .state
            .explorer
            .rows
            .iter()
            .any(|row| row.name == "new.txt");
        if !(reloaded && listed) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(reloaded, "buffer did not reload: {:?}", text(&app));
    assert!(listed, "explorer did not refresh");
    assert!(
        app.state.ui.message.contains("reloaded from disk"),
        "{}",
        app.state.ui.message
    );

    app.open_prompt(crate::input::PromptKind::QuickOpen);
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("quick-open prompt missing")
    };
    assert!(
        prompt
            .candidates
            .iter()
            .any(|candidate| candidate == "new.txt")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn external_documents_are_watched_outside_the_workspace_root() {
    use std::time::{Duration, Instant};

    let (mut app, dir) = fixture("one\n");
    let outside = std::env::temp_dir().join(format!("i3-external-watch-{}", std::process::id()));
    std::fs::remove_dir_all(&outside).ok();
    std::fs::create_dir_all(&outside).expect("outside");
    let external = outside.join("external.txt");
    std::fs::write(&external, "one\n").expect("external file");

    app.open_document(external.clone()).expect("open external");
    app.pump_watcher();
    assert!(app.external_watcher.is_some());
    assert!(
        app.external_watch_dirs
            .contains(&outside.canonicalize().expect("canonical")),
        "{:?}",
        app.external_watch_dirs
    );

    std::fs::write(&external, "two\n").expect("external write");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && text(&app) != "two\n" {
        app.pump_watcher();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(text(&app), "two\n");

    app.dispatch(Command::CloseDocument).expect("close");
    app.pump_watcher();
    assert!(app.external_watch_dirs.is_empty());
    assert!(app.external_watcher.is_none());
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn external_change_to_a_dirty_buffer_is_reported_not_reloaded() {
    let (mut app, dir) = fixture("one\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::InsertText("two ".to_string()))
        .expect("type");
    std::fs::write(&path, "outside\n").expect("external write");

    app.handle_external_change(&path);
    assert_eq!(text(&app), "two one\n", "dirty buffer must survive");
    assert!(app.state.active_document().expect("doc").dirty);
    assert!(
        app.state.ui.message.contains("changed on disk"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn deleting_an_open_file_is_reported() {
    let (mut app, dir) = fixture("one\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    std::fs::remove_file(&path).expect("remove");

    app.handle_external_change(&path);
    assert_eq!(text(&app), "one\n");
    assert!(
        app.state.ui.message.contains("deleted on disk"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn enter_inserts_the_detected_line_ending() {
    let (mut app, dir) = fixture("x\r\n");
    let id = app.state.active_document_id().expect("doc");
    app.state.documents.get_mut(&id).expect("doc").eol = Eol::Crlf;

    app.dispatch(Command::InsertNewline).expect("newline");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    app.dispatch(Command::Save).expect("save");

    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    assert_eq!(std::fs::read(&path).expect("read"), b"\r\nyx\r\n".to_vec());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn replace_in_files_previews_then_writes_one_file_at_a_time() {
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("needle one\n");
    let first = dir.join("note.txt");
    let second = dir.join("second.txt");
    std::fs::write(&second, "needle two\n").expect("file");

    app.dispatch(Command::ReplaceInFiles).expect("open");
    for ch in "needle".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.pump_project_search();
        if app.project_task.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.project_task.is_none(), "search did not finish");
    assert_eq!(
        std::fs::read_to_string(&first).expect("first"),
        "needle one\n",
        "preview must not write"
    );
    assert_eq!(
        std::fs::read_to_string(&second).expect("second"),
        "needle two\n",
        "preview must not write"
    );

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for ch in "done".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        std::fs::read_to_string(&first).expect("first"),
        "done one\n"
    );
    assert_eq!(
        std::fs::read_to_string(&second).expect("second"),
        "needle two\n"
    );
    assert!(
        matches!(app.state.mode, InputMode::Prompt(_)),
        "prompt closes after the first file"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.pump_project_search();
        if app.project_task.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("prompt missing")
    };
    assert_eq!(prompt.candidates.len(), 1, "{:?}", prompt.candidates);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        std::fs::read_to_string(&second).expect("second"),
        "done two\n"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn replace_in_files_refuses_a_dirty_open_buffer() {
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("needle\n");
    let path = dir.join("note.txt");
    app.dispatch(Command::InsertText("x".to_string()))
        .expect("type");
    assert!(app.state.active_document().expect("doc").dirty);

    app.dispatch(Command::ReplaceInFiles).expect("open");
    for ch in "needle".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.pump_project_search();
        if app.project_task.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for ch in "done".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "needle\n",
        "a dirty buffer must not be overwritten"
    );
    assert!(
        app.state.ui.message.contains("unsaved changes"),
        "{}",
        app.state.ui.message
    );
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn go_to_symbol_jumps_to_the_definition() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("fn alpha() {\n    one();\n}\n\nfn beta() {}\n");
    let id = app.state.active_document_id().expect("doc");
    app.state.documents.get_mut(&id).expect("doc").path = Some(dir.join("note.rs"));

    app.dispatch(Command::GoToSymbol).expect("open symbols");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("symbol prompt missing")
    };
    assert_eq!(prompt.candidates.len(), 2, "{:?}", prompt.candidates);
    for ch in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let cursor = app.state.focused_pane().expect("pane").cursor;
    let document = app.state.active_document().expect("doc");
    assert_eq!(document.line_of_char(cursor), 4);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn toggle_fold_hides_the_block_and_an_edit_clears_it() {
    let (mut app, dir) = fixture("fn alpha() {\n    one();\n    two();\n}\n\nafter\n");
    let id = app.state.active_document_id().expect("doc");
    app.state.documents.get_mut(&id).expect("doc").path = Some(dir.join("note.rs"));

    app.dispatch(Command::ToggleFold).expect("fold");
    {
        let document = app.state.active_document().expect("doc");
        assert!(document.is_hidden(1), "folded");
        assert!(document.is_hidden(3), "folded");
        assert!(!document.is_hidden(4), "blank line visible");
        assert_eq!(document.folded.len(), 1);
    }
    assert!(
        app.state.ui.message.contains("folded 3"),
        "{}",
        app.state.ui.message
    );

    app.dispatch(Command::InsertText("x".to_string()))
        .expect("type");
    assert!(app.state.active_document().expect("doc").folded.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn workspaces_keep_their_own_roots_and_quick_open_lists() {
    let (mut app, dir) = fixture("one\n");
    let other = std::env::temp_dir().join(format!("i3-ws-other-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&other);
    std::fs::create_dir_all(&other).expect("other dir");
    std::fs::write(other.join("extra.txt"), "other\n").expect("extra");
    let dir_root = std::fs::canonicalize(&dir).expect("canonical dir");
    let other_root = std::fs::canonicalize(&other).expect("canonical other");

    app.dispatch(Command::NewWorkspace).expect("new workspace");
    assert_eq!(app.state.workspaces.active, 1);
    app.dispatch(Command::SetWorkspaceRoot)
        .expect("root prompt");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.set_input(other.to_string_lossy().into_owned());
    }
    app.submit_prompt().expect("set root");
    assert_eq!(app.state.explorer.root, other_root);

    app.dispatch(Command::SwitchWorkspace(0))
        .expect("first workspace");
    assert_eq!(app.state.explorer.root, dir_root);
    let files = app.quick_open_candidates();
    assert!(files.contains(&"note.txt".to_string()), "{files:?}");
    assert!(!files.contains(&"other/extra.txt".to_string()), "{files:?}");

    app.dispatch(Command::SwitchWorkspace(1))
        .expect("second workspace");
    assert_eq!(app.state.explorer.root, other_root);
    let files = app.quick_open_candidates();
    assert!(files.contains(&"extra.txt".to_string()), "{files:?}");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&other).ok();
}

#[test]
fn crash_recovery_restores_a_dirty_buffer_and_can_save_it() {
    let (mut app, dir) = fixture("one\n");
    let path = dir.join("note.txt");
    app.dispatch(Command::InsertText("x".to_string()))
        .expect("type");
    let swap = dir.join("swap");
    app.swap_dir = Some(swap.clone());
    app.flush_swaps();

    let mut documents = std::collections::HashMap::new();
    let mut ids = crate::state::Ids::default();
    let recovered = crate::recovery::recover_from(&swap, &mut documents, &mut ids);
    assert_eq!(recovered.len(), 1);
    let id = recovered[0];
    let document = documents.remove(&id).expect("recovered document");
    assert_eq!(document.rope.to_string(), "xone\n");
    assert!(document.dirty);
    assert_eq!(document.path.as_deref(), Some(path.as_path()));

    app.state.documents.insert(id, document);
    app.recoveries = vec![id];
    app.begin_recovery_prompt();
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("recovery prompt missing")
    };
    assert!(prompt.candidates[0].starts_with("Save recovered buffer"));

    app.submit_prompt().expect("save recovery");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "xone\n");
    assert!(!app.state.documents[&id].dirty);
    assert!(
        std::fs::read_dir(&swap)
            .expect("swap dir")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".swap"))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lsp_diagnostics_completion_hover_and_definition_reach_the_ui() {
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::{Value, json};

    use crate::lsp::{LspManager, read_frame, write_frame};

    let (mut app, dir) = fixture("fn main() {}\n");
    let id = app.state.active_document_id().expect("doc");
    let source = dir.join("main.rs");
    std::fs::write(&source, "fn main() {}\n").expect("source");
    app.state.documents.get_mut(&id).expect("doc").path = Some(source.clone());
    let target = dir.join("other.rs");
    std::fs::write(&target, "fn other() {}\n").expect("target");
    let target_uri = crate::lsp::uri(&target);

    let mut config = crate::config::LspConfig::default();
    config.servers.insert(
        "fake".to_string(),
        crate::config::LspServerConfig {
            command: "fake".to_string(),
            args: Vec::new(),
            extensions: vec!["rs".to_string()],
        },
    );
    let mut manager = LspManager::new(&config, dir.clone());
    let (stream, peer) = UnixStream::pair().expect("pair");
    manager.attach_for_test("fake", stream).expect("attach");
    std::thread::spawn(move || {
        let read = peer.try_clone().expect("clone");
        let mut reader = BufReader::new(read);
        let mut writer = peer;
        while let Some(message) = read_frame(&mut reader) {
            let reply: Option<Value> = match message.get("method").and_then(Value::as_str) {
                Some("initialize") => Some(json!({
                    "jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}
                })),
                Some("textDocument/didOpen") => Some(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": message["params"]["textDocument"]["uri"],
                        "diagnostics": [{
                            "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}},
                            "severity": 1,
                            "message": "unknown name"
                        }]
                    }
                })),
                Some("textDocument/completion") => Some(json!({
                    "jsonrpc": "2.0", "id": message["id"],
                    "result": {"items": [{"label": "answer", "detail": "u32", "insertText": "answer"}]}
                })),
                Some("textDocument/hover") => Some(json!({
                    "jsonrpc": "2.0", "id": message["id"],
                    "result": {"contents": {"kind": "plaintext", "value": "u32"}}
                })),
                Some("textDocument/definition") => Some(json!({
                    "jsonrpc": "2.0", "id": message["id"],
                    "result": [{
                        "uri": target_uri,
                        "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 8}}
                    }]
                })),
                _ => None,
            };
            if let Some(reply) = reply {
                write_frame(&mut writer, &reply).expect("reply");
            }
        }
    });
    app.lsp = Some(manager);

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !app.state.lsp_diagnostics.contains_key(&id) {
        app.pump_lsp();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.state.lsp_diagnostics[&id][0].message, "unknown name");

    app.dispatch(Command::LspCompletion).expect("completion");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && app.state.completions.is_none() {
        app.pump_lsp();
        std::thread::sleep(Duration::from_millis(10));
    }
    let popup = app.state.completions.as_ref().expect("completion popup");
    assert_eq!(popup.items[0].label, "answer");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(text(&app).starts_with("answerfn main"), "{}", text(&app));
    assert!(app.state.completions.is_none());

    app.dispatch(Command::LspHover).expect("hover");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && app.state.hover.is_none() {
        app.pump_lsp();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.state.hover.as_deref(), Some("u32"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.state.hover.is_none());

    app.dispatch(Command::LspDefinition).expect("definition");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.pump_lsp();
        if app
            .state
            .active_document()
            .and_then(|document| document.path.clone())
            .is_some_and(|path| path == std::fs::canonicalize(&target).expect("canonical"))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let document = app.state.active_document().expect("definition target");
    assert_eq!(
        document.path.as_deref(),
        Some(target.canonicalize().expect("canonical").as_path())
    );
    let cursor = app.state.focused_pane().expect("pane").cursor;
    assert_eq!(document.line_of_char(cursor), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn workspace_root_change_restarts_lsp() {
    use crate::config::{LspConfig, LspServerConfig};
    use crate::lsp::LspManager;

    let (mut app, dir) = fixture("x\n");
    let mut config = LspConfig::default();
    config.servers.insert(
        "fake".to_string(),
        LspServerConfig {
            command: "fake".to_string(),
            args: Vec::new(),
            extensions: vec!["rs".to_string()],
        },
    );
    app.state.config.lsp = config.clone();
    app.lsp = Some(LspManager::new(&config, dir.clone()));
    let id = app.state.active_document_id().expect("doc");
    app.state.lsp_diagnostics.insert(id, Vec::new());

    let root = dir.join("other");
    std::fs::create_dir_all(&root).expect("root");
    app.set_workspace_root(root.clone()).expect("set root");

    let manager = app.lsp.as_ref().expect("lsp");
    assert_eq!(
        manager.root_for_test(),
        root.canonicalize().expect("canonical root").as_path()
    );
    assert!(app.state.lsp_diagnostics.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_reload_restarts_lsp_with_new_servers() {
    use crate::config::LspConfig;
    use crate::lsp::LspManager;

    let (mut app, dir) = fixture("x\n");
    let config_path = dir.join("config.toml");
    std::fs::write(
        &config_path,
        "[lsp.fake]\ncommand = \"fake\"\nextensions = [\"rs\"]\n",
    )
    .expect("config");
    app.state.config_path = Some(config_path);
    app.lsp = Some(LspManager::new(&LspConfig::default(), dir.clone()));
    assert!(!app.lsp.as_ref().expect("lsp").is_configured());

    app.reload_config();
    assert!(app.lsp.as_ref().expect("lsp").is_configured());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_stage_hunk_stages_and_unstages_only_the_selected_hunk() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::process::Command as Process;

    if Process::new("git").arg("--version").output().is_err() {
        return;
    }
    let base: String = (1..=20).map(|number| format!("line {number}\n")).collect();
    let (mut app, dir) = fixture(&base);
    let path = dir.join("note.txt");
    let git = |args: &[&str]| {
        let status = Process::new("git")
            .args(args)
            .current_dir(&dir)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    let output = |args: &[&str]| {
        let output = Process::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("git");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);
    let changed = base
        .replace("line 2\n", "line 2 changed\n")
        .replace("line 18\n", "line 18 changed\n");
    std::fs::write(&path, &changed).expect("edit");

    app.dispatch(Command::GitStageHunk).expect("hunk picker");
    let picker = app.state.hunks.as_ref().expect("picker");
    assert_eq!(picker.hunks.len(), 2);
    assert!(picker.stage);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.state.hunks.is_none());

    let cached = output(&["diff", "--cached", "--no-color"]);
    assert!(cached.contains("+line 18 changed"), "{cached}");
    assert!(!cached.contains("+line 2 changed"), "{cached}");

    app.dispatch(Command::GitUnstageHunk)
        .expect("unstage picker");
    assert_eq!(app.state.hunks.as_ref().expect("picker").hunks.len(), 1);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        output(&["diff", "--cached", "--no-color"])
            .trim()
            .is_empty()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_log_overlay_shows_branch_branches_and_commits() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::process::Command as Process;

    if Process::new("git").arg("--version").output().is_err() {
        return;
    }
    let (mut app, dir) = fixture("one\n");
    let git = |args: &[&str]| {
        let status = Process::new("git")
            .args(args)
            .current_dir(&dir)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);
    git(&["branch", "topic"]);

    app.dispatch(Command::GitLog).expect("git log");
    let view = app.state.git_diff.as_ref().expect("git log overlay");
    assert!(view.title.contains("branches and log"), "{}", view.title);
    let text = view.lines.join("\n");
    assert!(text.contains("Branch:"), "{text}");
    assert!(text.contains("topic"), "{text}");
    assert!(text.contains("base"), "{text}");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.state.git_diff.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_pull_refuses_unsaved_buffers_before_touching_git() {
    use std::process::Command as Process;

    if Process::new("git").arg("--version").output().is_err() {
        return;
    }
    let (mut app, dir) = fixture("one\n");
    let status = Process::new("git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .status()
        .expect("git init");
    assert!(status.success());
    app.dispatch(Command::InsertText("edited ".to_string()))
        .expect("dirty buffer");
    app.dispatch(Command::GitPull).expect("pull refused");
    assert!(
        app.state.ui.message.contains("unsaved changes"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn resolve_conflict_ours_theirs_and_both_are_single_edits() {
    let source = "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\nafter\n";
    let (mut app, dir) = fixture(source);
    let pane_id = app.state.focused_pane_id().expect("pane");

    app.state.panes.get_mut(&pane_id).expect("pane").cursor = 0;
    app.dispatch(Command::ResolveConflictOurs)
        .expect("resolve ours");
    assert_eq!(text(&app), "before\nours\nafter\n");
    app.dispatch(Command::Undo).expect("undo ours");
    assert_eq!(text(&app), source);

    app.state.panes.get_mut(&pane_id).expect("pane").cursor = 0;
    app.dispatch(Command::ResolveConflictTheirs)
        .expect("resolve theirs");
    assert_eq!(text(&app), "before\ntheirs\nafter\n");
    app.dispatch(Command::Undo).expect("undo theirs");
    assert_eq!(text(&app), source);

    app.state.panes.get_mut(&pane_id).expect("pane").cursor = 0;
    app.dispatch(Command::ResolveConflictBoth)
        .expect("resolve both");
    assert_eq!(text(&app), "before\nours\ntheirs\nafter\n");
    assert!(
        app.state.ui.message.contains("all conflicts resolved"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn conflict_prompt_opens_the_first_marker_after_a_merge() {
    use std::process::Command as Process;

    if Process::new("git").arg("--version").output().is_err() {
        return;
    }
    let (mut app, dir) = fixture("base\n");
    let git = |args: &[&str]| {
        Process::new("git")
            .args(args)
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["config", "user.email", "test@example.com"]).success());
    assert!(git(&["config", "user.name", "Test"]).success());
    assert!(git(&["add", "."]).success());
    assert!(git(&["commit", "-q", "-m", "base"]).success());
    let main = crate::git::branch("git", &dir).expect("main branch");

    assert!(git(&["checkout", "-q", "-b", "side"]).success());
    std::fs::write(dir.join("note.txt"), "side\n").expect("side");
    assert!(git(&["add", "."]).success());
    assert!(git(&["commit", "-q", "-m", "side"]).success());
    assert!(git(&["checkout", "-q", &main]).success());
    std::fs::write(dir.join("note.txt"), "main\n").expect("main");
    assert!(git(&["add", "."]).success());
    assert!(git(&["commit", "-q", "-m", "main"]).success());
    assert!(!git(&["merge", "side"]).success(), "merge must conflict");

    app.dispatch(Command::GitConflicts)
        .expect("conflict prompt");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("conflict prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::Conflicts);
    assert!(
        prompt
            .candidates
            .iter()
            .any(|line| line.contains("note.txt")),
        "{:?}",
        prompt.candidates
    );
    app.submit_prompt().expect("open conflict");

    let document = app.state.active_document().expect("conflict file");
    let expected = dir.join("note.txt").canonicalize().expect("canonical");
    assert_eq!(document.path.as_deref(), Some(expected.as_path()));
    let text = text(&app);
    assert!(text.contains("<<<<<<<"), "{text}");
    let marker = text
        .lines()
        .position(|line| line.starts_with("<<<<<<<"))
        .expect("marker line");
    let cursor = app.state.focused_pane().expect("pane").cursor;
    assert_eq!(document.line_of_char(cursor), marker);
    assert!(
        app.state.ui.message.contains("conflict at"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_markers_diff_staging_and_commit_work() {
    use std::process::Command as Process;

    if Process::new("git").arg("--version").output().is_err() {
        return;
    }
    let (mut app, dir) = fixture("one\ntwo\n");
    let git = |args: &[&str]| {
        let status = Process::new("git")
            .args(args)
            .current_dir(&dir)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    let output = |args: &[&str]| {
        let output = Process::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("git");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    let path = dir.join("note.txt");
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);

    app.dispatch(Command::InsertText("x".to_string()))
        .expect("type");
    app.dispatch(Command::Save).expect("save");
    app.refresh_git_marks();
    let id = app.state.active_document_id().expect("doc");
    let marks = app.state.git_marks.get(&id).expect("marks");
    assert_eq!(marks[0], Some(crate::git::Change::Modified), "{marks:?}");

    app.dispatch(Command::GitDiff).expect("diff");
    let view = app.state.git_diff.as_ref().expect("diff view");
    assert!(view.title.contains("note.txt"), "{}", view.title);
    assert!(view.lines.iter().any(|line| line == "+xone"), "{view:?}");
    assert_eq!(std::fs::read_to_string(&path).expect("file"), "xone\ntwo\n");
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Esc,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(app.state.git_diff.is_none());

    app.dispatch(Command::GitStage).expect("stage");
    assert_eq!(output(&["diff", "--cached", "--name-only"]), "note.txt");
    app.dispatch(Command::GitUnstage).expect("unstage");
    assert_eq!(output(&["diff", "--name-only"]), "note.txt");

    app.dispatch(Command::GitStage).expect("stage again");
    app.dispatch(Command::GitCommit).expect("commit prompt");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.set_input("second commit");
    }
    app.submit_prompt().expect("commit");
    assert_eq!(output(&["log", "-1", "--pretty=%s"]), "second commit");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_commands_report_a_missing_repository() {
    let (mut app, dir) = fixture("one\n");
    let result = app.dispatch(Command::GitDiff);
    app.finish(result);
    assert!(
        app.state.ui.message.contains("not a git repository"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn multi_cursor_edits_and_undoes_once() {
    let (mut app, dir) = fixture("one\ntwo\n");
    app.dispatch(Command::AddCursorBelow).expect("add cursor");
    let pane = app.state.focused_pane().expect("pane");
    assert_eq!(pane.extra.len(), 1, "{pane:?}");

    app.dispatch(Command::InsertText("X".to_string()))
        .expect("type");
    assert_eq!(text(&app), "Xone\nXtwo\n");
    let pane = app.state.focused_pane().expect("pane");
    assert_eq!(pane.extra.len(), 1);

    app.dispatch(Command::Undo).expect("undo");
    assert_eq!(text(&app), "one\ntwo\n");
    assert!(app.state.focused_pane().expect("pane").extra.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn column_selection_copies_cuts_and_undoes_once() {
    let (mut app, dir) = fixture("one\ntwo\n");
    let id = app.state.active_document_id().expect("doc");
    let pane_id = app.state.focused_pane_id().expect("pane");
    {
        let document = app.state.documents.get(&id).expect("doc");
        let pane = app.state.panes.get_mut(&pane_id).expect("pane");
        pane.column_selection(document, 0, 0, 1, 1);
    }
    assert_eq!(app.state.focused_pane().expect("pane").extra.len(), 1);

    app.dispatch(Command::Copy).expect("copy");
    assert_eq!(app.state.clipboard, "o\nt");

    app.dispatch(Command::Cut).expect("cut");
    assert_eq!(text(&app), "ne\nwo\n");
    app.dispatch(Command::Undo).expect("undo");
    assert_eq!(text(&app), "one\ntwo\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wrapped_cursor_movement_and_click_use_visual_rows() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let line = "word ".repeat(40);
    let source = format!("{line}\n{line}\n");
    let (mut app, dir) = fixture(&source);
    app.state.config.editor.word_wrap = true;
    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    let geometry = {
        let pane = app.state.panes.get(&pane_id).expect("pane");
        let document = app.state.active_document().expect("doc");
        crate::ui::pane::text_geometry(&app.state, pane, document, rect).expect("geometry")
    };

    app.dispatch(Command::MoveCursor(crate::command::Move::Down))
        .expect("down");
    let cursor = app.state.focused_pane().expect("pane").cursor;
    let document = app.state.active_document().expect("doc");
    assert_eq!(document.line_of_char(cursor), 0, "moved to the next line");
    assert!(cursor > 0, "moved within the wrapped line");

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: geometry.area.x + geometry.gutter + 5,
        row: geometry.area.y + 1,
        modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
    });
    let cursor = app.state.focused_pane().expect("pane").cursor;
    let document = app.state.active_document().expect("doc");
    assert_eq!(document.line_of_char(cursor), 0, "clicked the wrapped row");
    assert!(
        cursor - document.char_of_line(0) > 20,
        "click landed on the first visual row"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wrapped_movement_crosses_lines_and_skips_folds() {
    let source = format!(
        "{}\n{}\n{}\n",
        "a".repeat(100),
        "b".repeat(100),
        "c".repeat(100)
    );
    let (mut app, dir) = fixture(&source);
    app.state.config.editor.word_wrap = true;
    let pane_id = app.state.focused_pane_id().expect("pane");
    let document_id = app.state.active_document_id().expect("doc");
    let start = {
        let document = app.state.documents.get(&document_id).expect("doc");
        document.char_of_line(0) + 100
    };
    {
        let pane = app.state.panes.get_mut(&pane_id).expect("pane");
        pane.cursor = start;
        pane.target_col = None;
    }

    app.dispatch(Command::MoveCursor(crate::command::Move::Down))
        .expect("down");
    {
        let document = app.state.documents.get(&document_id).expect("doc");
        let cursor = app.state.panes[&pane_id].cursor;
        assert_eq!(document.line_of_char(cursor), 1);
        assert!(cursor > document.char_of_line(1));
    }

    app.dispatch(Command::MoveCursor(crate::command::Move::Up))
        .expect("up");
    let cursor = app.state.panes[&pane_id].cursor;
    let document = app.state.documents.get(&document_id).expect("doc");
    assert_eq!(document.line_of_char(cursor), 0);
    assert!(cursor > document.char_of_line(0));

    // Folding line 1 must make Down jump from line 0 to line 2.
    app.state
        .documents
        .get_mut(&document_id)
        .expect("doc")
        .folded = std::iter::once(0..1).collect();
    {
        let pane = app.state.panes.get_mut(&pane_id).expect("pane");
        pane.cursor = start;
        pane.target_col = None;
    }
    app.dispatch(Command::MoveCursor(crate::command::Move::Down))
        .expect("down across fold");
    let cursor = app.state.panes[&pane_id].cursor;
    let document = app.state.documents.get(&document_id).expect("doc");
    assert_eq!(document.line_of_char(cursor), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn line_endings_convert_and_save_round_trip() {
    let (mut app, dir) = fixture("one\ntwo\n");
    let path = dir.join("note.txt");
    app.dispatch(Command::LineEndingsCrlf).expect("crlf");
    {
        let document = app.state.active_document().expect("doc");
        assert_eq!(document.eol, crate::editor::document::Eol::Crlf);
        assert!(document.rope.to_string().contains("\r\n"));
    }
    app.dispatch(Command::Save).expect("save");
    assert_eq!(
        std::fs::read(&path).expect("read"),
        b"one\r\ntwo\r\n".to_vec()
    );

    app.dispatch(Command::LineEndingsLf).expect("lf");
    app.dispatch(Command::Save).expect("save");
    assert_eq!(std::fs::read(&path).expect("read"), b"one\ntwo\n".to_vec());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn non_utf8_files_are_rejected_with_a_message() {
    let (mut app, dir) = fixture("one\n");
    let path = dir.join("bad.bin");
    std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("write");
    let error = app.open_document(path).expect_err("invalid utf8");
    app.finish(Err(error));
    assert!(
        app.state.ui.message.contains("not valid UTF-8"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lua_plugins_register_palette_commands_keymaps_hooks_and_status() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::config::LuaConfig;
    use crate::plugins::Plugins;

    let (mut app, dir) = fixture("one\n");
    let plugins_dir = dir.join("plugins");
    std::fs::create_dir_all(&plugins_dir).expect("plugin dir");
    let command_marker = dir.join("command.txt");
    let save_marker = dir.join("saved.txt");
    std::fs::write(
        plugins_dir.join("demo.lua"),
        format!(
            r#"
commands.register("plugin_hello", function()
  fs.write_file("{}", "ran")
end)
keymap.set("ctrl+alt+l", "plugin_hello")
events.on("save", function()
  fs.write_file("{}", editor.language())
end)
statusline.register(function()
  return "LUA:" .. editor.language()
end)
"#,
            command_marker.display(),
            save_marker.display()
        ),
    )
    .expect("plugin");
    let config = LuaConfig {
        enabled: true,
        allow_file: true,
        allow_process: false,
        time_budget_ms: 100,
    };
    let mut plugins = Plugins::new(plugins_dir.clone(), &config).expect("plugins");
    assert!(plugins.take_warnings().is_empty());
    app.plugins = Some(plugins);
    app.refresh_plugin_status();
    assert_eq!(app.state.plugin_status, vec!["LUA:plaintext"]);

    app.open_prompt(crate::input::PromptKind::CommandPalette);
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("palette missing")
    };
    assert!(prompt.candidates.contains(&"plugin_hello".to_string()));
    prompt.set_input("plugin_hello");
    prompt.refilter();
    app.submit_prompt().expect("run plugin command");
    assert_eq!(
        std::fs::read_to_string(&command_marker).expect("marker"),
        "ran"
    );

    std::fs::remove_file(&command_marker).expect("reset");
    app.handle_key(KeyEvent::new(
        KeyCode::Char('l'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ));
    assert_eq!(
        std::fs::read_to_string(&command_marker).expect("key marker"),
        "ran"
    );

    app.dispatch(Command::Save).expect("save");
    assert_eq!(
        std::fs::read_to_string(&save_marker).expect("save marker"),
        "plaintext"
    );

    std::fs::write(plugins_dir.join("demo.lua"), "-- removed\n").expect("rewrite");
    app.reload_plugins();
    assert!(app.plugin_command_names().is_empty());
    assert!(app.state.plugin_status.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn help_lists_active_bindings_and_closes_with_escape() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::Help).expect("help");
    let help = app.state.help.as_ref().expect("help overlay");
    assert!(
        help.lines
            .iter()
            .any(|line| line.contains("Ctrl+Q") && line.contains("quit")),
        "{:?}",
        help.lines
    );
    assert!(help.lines.iter().any(|line| line == "Global"));

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.state.help.as_ref().expect("help").scroll, 1);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.state.help.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn search_history_cycles_and_persists() {
    use crate::input::{KeyCombo, PromptKind};
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    let (mut app, dir) = fixture("alpha beta\n");
    let history_path = dir.join("history.toml");
    app.history_path = Some(history_path.clone());
    for query in ["alpha", "beta"] {
        app.open_prompt(PromptKind::Find);
        if let InputMode::Prompt(prompt) = &mut app.state.mode {
            prompt.set_input(query);
        }
        app.submit_prompt().expect("search");
    }

    app.open_prompt(PromptKind::Find);
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        assert_eq!(
            prompt.history.entries,
            vec!["beta".to_string(), "alpha".to_string()]
        );
        prompt.set_input("");
        prompt.history.index = None;
    } else {
        panic!("find prompt missing");
    }

    let previous = KeyCombo {
        code: KeyCode::Up,
        mods: KeyModifiers::ALT,
    };
    app.handle_prompt_key(previous);
    assert_eq!(prompt_input(&app), "beta");
    app.handle_prompt_key(previous);
    assert_eq!(prompt_input(&app), "alpha");
    app.handle_prompt_key(KeyCombo {
        code: KeyCode::Down,
        mods: KeyModifiers::ALT,
    });
    assert_eq!(prompt_input(&app), "beta");

    let saved = std::fs::read_to_string(&history_path).expect("history file");
    assert!(saved.contains("beta"), "{saved}");
    app.cancel_prompt();
    std::fs::remove_dir_all(&dir).ok();
}

fn prompt_input(app: &App) -> String {
    match &app.state.mode {
        InputMode::Prompt(prompt) => prompt.input().to_string(),
        InputMode::Normal => panic!("prompt closed"),
    }
}

#[test]
fn session_autosave_writes_the_current_layout() {
    use std::time::{Duration, Instant};

    let (mut app, dir) = fixture("x\n");
    let path = dir.join("session.toml");
    app.session_path = Some(path.clone());
    app.last_session_save = Instant::now()
        .checked_sub(Duration::from_secs(10))
        .expect("instant");
    app.autosave_session();

    let saved = std::fs::read_to_string(&path).expect("session file");
    assert!(saved.contains("note.txt"), "{saved}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scratch_buffer_can_be_saved_as_a_file() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::NewScratch).expect("new scratch");
    let document = app.state.active_document().expect("scratch");
    assert!(document.path.is_none());
    assert_eq!(document.display_name(), "[No Name]");

    app.dispatch(Command::InsertText("hello".to_string()))
        .expect("type");
    assert!(app.state.active_document().expect("scratch").dirty);

    let path = dir.join("scratch.txt");
    app.dispatch(Command::SavePrompt).expect("save as");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.fields[0] = path.to_string_lossy().into_owned();
    }
    app.submit_prompt().expect("save scratch");

    let document = app.state.active_document().expect("scratch");
    assert_eq!(document.path.as_deref(), Some(path.as_path()));
    assert_eq!(
        std::fs::read_to_string(&path).expect("scratch file"),
        "hello"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn workspace_terminal_directory_controls_new_shells() {
    use crate::input::PromptKind;

    let (mut app, dir) = fixture("x\n");
    let terminal_dir = dir.join("terminal");
    std::fs::create_dir_all(&terminal_dir).expect("terminal dir");

    app.dispatch(Command::SetWorkspaceTerminalDir)
        .expect("terminal directory prompt");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("terminal directory prompt missing")
    };
    assert_eq!(prompt.kind, PromptKind::WorkspaceTerminalDir);
    prompt.fields[0] = terminal_dir.to_string_lossy().into_owned();
    app.submit_prompt().expect("set terminal directory");
    assert_eq!(
        app.state.workspaces.active().terminal_dir,
        terminal_dir.canonicalize().expect("canonical")
    );

    app.dispatch(Command::TerminalNew).expect("new terminal");
    assert_eq!(
        app.state.terminals.active().expect("shell").cwd(),
        terminal_dir.canonicalize().expect("canonical").as_path()
    );
    app.state.terminals = crate::shell::Terminals::default();
    std::fs::remove_dir_all(&dir).ok();
}
