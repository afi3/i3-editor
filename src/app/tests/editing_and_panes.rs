use super::*;

#[test]
fn markdown_tables_align_and_style_inline_cells() {
    use ratatui::style::Modifier;
    let (mut app, dir) = fixture("x");
    app.state.ui.ai.width = 40;
    app.state.ai.entries = vec!["| Name | Qty |\n|:---|---:|\n| **a** | 2 |".to_string()];
    let rows = crate::ui::ai::build_rows(&app.state);
    let rendered: Vec<String> = rows
        .iter()
        .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();
    assert_eq!(rendered[0], "│Name│Qty│");
    assert_eq!(rendered[1], "│────│───│");
    assert_eq!(rendered[2], "│a   │  2│");
    assert!(
        rows[2]
            .spans
            .iter()
            .any(|span| span.content == "a" && span.style.add_modifier.contains(Modifier::BOLD))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markdown_headings_and_escapes_render_literally() {
    use ratatui::style::Modifier;
    let (mut app, dir) = fixture("x");
    app.state.ui.ai.width = 40;
    app.state.ai.entries = vec!["## Title *x*".to_string(), "literal \\*star\\*".to_string()];
    let rows = crate::ui::ai::build_rows(&app.state);
    let rendered: Vec<String> = rows
        .iter()
        .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();
    assert_eq!(rendered, vec!["Title x", "literal *star*"]);
    assert!(rows[0].spans.iter().any(|span| {
        span.content == "Title " && span.style.add_modifier.contains(Modifier::BOLD)
    }));
    assert!(
        !rows[1]
            .spans
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::ITALIC))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn native_edit_waits_for_lsp_diagnostics() {
    let (mut app, dir) = fixture("x\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.native_diagnostics = Some(crate::app::DiagnosticWait {
        path: path.clone(),
        prefix: "edit ok".to_string(),
        started: std::time::Instant::now(),
    });
    app.state.ai.waiting = Some(crate::state::AiWaiting {
        id: "call-1".to_string(),
        server: crate::native::SERVER.to_string(),
        tool: "edit_file".to_string(),
        capability: "destructive".to_string(),
    });
    app.state.ai.pending.push_back(crate::state::AiToolCall {
        id: "call-2".to_string(),
        server: crate::native::SERVER.to_string(),
        tool: "read_file".to_string(),
        capability: "read-only".to_string(),
        arguments: serde_json::json!({"path": "note.txt"}),
    });
    app.finish_native_diagnostics(
        path,
        vec![crate::lsp::Diagnostic {
            line: 0,
            column: 1,
            severity: crate::lsp::Severity::Error,
            message: "bad syntax".to_string(),
        }],
    );
    assert!(app.state.ai.conversation.iter().any(|message| {
        message.get("content").and_then(|content| content.as_str())
            == Some("edit ok\n\nLSP diagnostics:\n1:2: error: bad syntax")
    }));
    assert!(app.state.ai.waiting.is_none());
    assert_eq!(app.state.ai.pending.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_session_budget_stops_before_a_request() {
    let (mut app, dir) = fixture("x");
    app.state.config.ai.max_session_tokens = 10;
    app.state.ai.prompt_tokens = 10;
    app.start_ai_request();
    assert!(app.ai_task.is_none());
    assert!(app.ai_stream.is_none());
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|entry| entry.contains("session token budget reached"))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_session_cost_budget_stops_before_a_request() {
    let (mut app, dir) = fixture("x");
    app.state.config.ai.max_session_cost_usd = 0.01;
    app.state.config.ai.input_cost_per_million = 1.0;
    app.state.ai.prompt_tokens = 10_000;
    app.start_ai_request();
    assert!(app.ai_task.is_none());
    assert!(app.ai_stream.is_none());
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|entry| entry.contains("session cost budget reached"))
    );
    std::fs::remove_dir_all(&dir).ok();
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

    let deadline = Instant::now() + Duration::from_secs(15);
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
fn block_comment_on_non_ascii_text_keeps_the_line_count() {
    let (mut app, dir) = fixture("h\u{e9}llo \u{2603}\nsecond\n");
    set_document_path(&mut app, "note.md");
    app.dispatch(Command::ToggleComment).expect("comment");
    assert_eq!(text(&app), "<!-- h\u{e9}llo \u{2603} -->\nsecond\n");
    app.dispatch(Command::ToggleComment).expect("uncomment");
    assert_eq!(text(&app), "h\u{e9}llo \u{2603}\nsecond\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn toggling_a_comment_in_an_empty_file_does_not_panic() {
    let (mut app, dir) = fixture("");
    set_document_path(&mut app, "empty.rs");
    app.dispatch(Command::ToggleComment).expect("comment");
    assert_eq!(text(&app), "//");
    app.dispatch(Command::ToggleComment).expect("uncomment");
    assert_eq!(text(&app), "");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn deleting_the_last_empty_line_of_a_crlf_file_drops_the_carriage_return() {
    use crate::command::Move;

    let (mut app, dir) = fixture("a\r\n");
    app.dispatch(Command::MoveCursor(Move::DocEnd))
        .expect("end");
    app.dispatch(Command::DeleteLine).expect("delete");
    assert_eq!(text(&app), "a");
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
fn ctrl_b_focuses_a_visible_explorer_before_hiding_it() {
    let (mut app, dir) = fixture("x\n");
    let pane = app.state.focused_pane_id().expect("pane");

    app.state.explorer.visible = false;
    app.dispatch(Command::ToggleExplorer).expect("show");
    assert!(app.state.explorer.visible);
    assert_eq!(app.state.focus, Focus::Explorer);

    app.state.focus = Focus::Pane(pane);
    app.dispatch(Command::ToggleExplorer).expect("focus");
    assert!(
        app.state.explorer.visible,
        "a visible tree is focused, not hidden"
    );
    assert_eq!(app.state.focus, Focus::Explorer);

    app.dispatch(Command::ToggleExplorer).expect("hide");
    assert!(!app.state.explorer.visible);
    assert!(matches!(app.state.focus, Focus::Pane(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn switching_documents_resets_a_stale_viewport() {
    let (mut app, dir) = fixture(&"line\n".repeat(200));
    let pane_id = app.state.focused_pane_id().expect("pane");
    let pane = app.state.panes.get_mut(&pane_id).expect("pane");
    pane.scroll_top = 150;
    pane.scroll_left = 3;
    app.dispatch(Command::NewScratch).expect("scratch");
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);
    assert_eq!(app.state.panes[&pane_id].scroll_left, 0);

    let pane = app.state.panes.get_mut(&pane_id).expect("pane");
    pane.scroll_top = 7;
    app.dispatch(Command::NextDocument).expect("next tab");
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dragging_a_split_separator_follows_the_pointer_once() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    crate::ui::compute_layout(&mut app.state, Rect::new(0, 0, 100, 30));
    let area = app.state.ui.panes;
    let mut separators = Vec::new();
    app.state
        .workspaces
        .active()
        .layout
        .separators(area, &mut Vec::new(), &mut separators);
    let separator = separators.first().expect("separator").clone();
    let row = separator.span_start;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: separator.at,
        row,
        modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
    });
    assert!(app.drag.is_some(), "separator drag did not start");
    for step in 1..=5u16 {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: separator.at - step,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        });
    }
    let mut after = Vec::new();
    app.state
        .workspaces
        .active()
        .layout
        .separators(area, &mut Vec::new(), &mut after);
    // One cell of slack for the weight rounding; compounding would land near
    // `separator.at - 15`.
    let moved = i32::from(separator.at) - i32::from(after[0].at);
    assert!(
        (4..=6).contains(&moved),
        "a 5-cell drag moved the border {moved} cells"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_paste_goes_to_the_focused_ai_composer() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("panel");
    app.state.ai.focused = true;
    app.state.ai.input = "ab".to_string();
    app.state.ai.cursor = 1;
    app.handle_paste("XY".to_string()).expect("paste");
    assert_eq!(app.state.ai.input, "aXYb");
    assert_eq!(app.state.ai.cursor, 3);
    assert_eq!(
        text(&app),
        "x\n",
        "the buffer behind the panel is untouched"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_paste_while_an_overlay_is_open_leaves_the_buffer_alone() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::Help).expect("help");
    assert!(app.state.help.is_some());
    app.handle_paste("oops".to_string()).expect("paste");
    assert_eq!(text(&app), "x\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_paste_does_not_reach_the_ai_composer_behind_a_modal() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("panel");
    app.state.ai.focused = true;
    app.state.ai.input = "keep".to_string();
    app.dispatch(Command::AiRules).expect("rules");
    assert!(app.state.rules_view.is_some());
    app.handle_paste("oops".to_string()).expect("paste");
    assert_eq!(app.state.ai.input, "keep");
    assert_eq!(text(&app), "x\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wheeling_the_explorer_does_not_take_keyboard_focus() {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};

    let (mut app, dir) = fixture("x\n");
    let before = app.state.focus;
    let rect = app.state.ui.explorer;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: rect.x + 1,
        row: rect.y + 2,
        modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
    });
    assert_eq!(app.state.focus, before, "the wheel must not move focus");
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

    let deadline = Instant::now() + Duration::from_secs(15);
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
            .rope()
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
    // 30 columns leave room for two minimum panes beside the explorer.
    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 30, 12));
    app.dispatch(Command::SplitLeftRight)
        .expect("two minimum panes fit in 30 columns");
    let refused = app.dispatch(Command::SplitLeftRight);
    assert!(
        refused.is_err(),
        "a third pane cannot fit in 30 columns: {:?}",
        app.state.panes.len()
    );
    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 30, 12));
    for rect in app.state.ui.pane_rects.values() {
        assert!(rect.width >= crate::workspace::MIN_PANE_COLS, "{rect:?}");
        assert!(rect.height >= crate::workspace::MIN_PANE_ROWS, "{rect:?}");
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
fn clicking_a_row_scrolled_by_wrap_offset_lands_on_the_drawn_row() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let source = "word ".repeat(400);
    let (mut app, dir) = fixture(&source);
    app.state.config.editor.word_wrap = true;
    crate::ui::compute_layout(&mut app.state, Rect::new(0, 0, 60, 14));
    for _ in 0..40 {
        app.dispatch(Command::MoveCursor(crate::command::Move::Down))
            .expect("down");
    }
    let pane_id = app.state.focused_pane_id().expect("pane");
    let scroll_row = app.state.panes[&pane_id].scroll_row;
    let scroll_top = app.state.panes[&pane_id].scroll_top;
    assert!(scroll_row > 0, "the line must be scrolled by row");
    let geometry = {
        let pane = &app.state.panes[&pane_id];
        let document = app.state.active_document().expect("doc");
        crate::ui::pane::text_geometry(
            &app.state,
            pane,
            document,
            app.state.ui.pane_rects[&pane_id],
        )
        .expect("geometry")
    };
    let expected = {
        let document = app.state.active_document().expect("doc");
        let rows = crate::editor::text::visual_rows(
            document,
            scroll_top,
            geometry.text_width,
            document.indent.tab_width.max(1),
            scroll_row + 1,
        );
        rows[scroll_row]
    };
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: geometry.area.x + geometry.gutter + 2,
        row: geometry.area.y,
        modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
    });
    let cursor = app.state.panes[&pane_id].cursor;
    let document = app.state.active_document().expect("doc");
    assert_eq!(document.line_of_char(cursor), expected.line);
    let column = cursor - document.char_of_line(expected.line);
    assert!(
        column >= expected.start && column <= expected.end,
        "clicked char {column} outside the drawn row {expected:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn closing_a_tabbed_pane_focuses_a_visible_one() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SplitLeftRight).expect("split");
    app.dispatch(Command::ToggleTabbed).expect("tabbed");
    app.dispatch(Command::NextPaneTab).expect("second tab");
    app.dispatch(Command::ClosePane).expect("close");
    let focused = app.state.focused_pane_id().expect("focus");
    assert!(
        app.state.ui.pane_rects.contains_key(&focused),
        "focus landed on an inactive tab: {focused:?}"
    );
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
