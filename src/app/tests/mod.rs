use std::path::PathBuf;

use crate::command::Command;
use crate::config::AppConfig;
use crate::editor::document::{Document, Eol};
use crate::explorer::ExplorerState;
use crate::input::KeyMap;
use crate::state::{AppState, Focus, Ids, InputMode, Pane};
use crate::ui;
use crate::workspace::{Workspace, WorkspaceManager};

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
    let mut document = Document::from_text(document_id, text, Some(path.clone()));
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
        mcp_view: None,
        rules_view: None,
        trace_view: None,
        ai: crate::state::AiPanel::default(),
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
        .map(|document| document.rope().to_string())
        .unwrap_or_default()
}

fn cursor(app: &App) -> usize {
    app.state
        .focused_pane()
        .map(|pane| pane.cursor)
        .unwrap_or(0)
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

const MIN_COLS: u16 = crate::workspace::MIN_PANE_COLS;

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

fn prompt_input(app: &App) -> String {
    match &app.state.mode {
        InputMode::Prompt(prompt) => prompt.input().to_string(),
        InputMode::Normal => panic!("prompt closed"),
    }
}

// This was one 5,500-line module. The slices keep their original order; each
// module is named for the area its tests mostly cover.
mod ai_and_mcp;
mod editing_and_panes;
mod input_and_search;
mod integrations;
