use super::*;

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
fn scrollbar_click_and_drag_jump_through_the_document() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let text = (0..200).map(|n| format!("line {n}\n")).collect::<String>();
    let (mut app, dir) = fixture(&text);
    let pane_id = app.state.focused_pane_id().expect("pane");
    let geometry = text_geometry(&app);
    assert!(geometry.scrollbar);
    let x = geometry.area.x + geometry.area.width - 1;
    let bottom = geometry.area.y + geometry.area.height - 1;

    click(&mut app, x, bottom, KeyModifiers::NONE);
    let max_top = app
        .state
        .active_document()
        .expect("document")
        .line_count()
        .saturating_sub(usize::from(geometry.area.height));
    assert_eq!(app.state.panes[&pane_id].scroll_top, max_top);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: x,
        row: geometry.area.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x,
        row: geometry.area.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.scrollbar_drag.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn qterminal_backspace_and_delete_sequences_edit_text() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("abc");
    app.dispatch(Command::MoveCursor(crate::command::Move::DocEnd))
        .expect("end");
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert_eq!(text(&app), "ab");

    app.dispatch(Command::MoveCursor(crate::command::Move::DocStart))
        .expect("start");
    app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT));
    assert_eq!(text(&app), "b");
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

    let before = app.state.focus;
    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.explorer.selected, 3);
    assert_eq!(app.state.focus, before, "the wheel must not take focus");

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
fn clicking_a_tab_close_button_closes_that_document() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("one\n");
    let first = app.state.active_document().expect("first").id;
    let other = dir.join("other.txt");
    std::fs::write(&other, "two\n").expect("other");
    app.open_document(other).expect("open other");

    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(rect);
    let tabs = crate::ui::pane::document_tabs(&app.state, &app.state.panes[&pane_id], 4);
    let first = tabs.iter().find(|tab| tab.id == first).expect("first tab");
    click(
        &mut app,
        inner.x + first.close_start(),
        inner.y,
        KeyModifiers::NONE,
    );

    assert!(!app.state.documents.contains_key(&first.id));
    assert_eq!(app.state.panes[&pane_id].tabs.len(), 1);
    assert_eq!(
        app.state
            .active_document()
            .map(|document| document.display_name()),
        Some("other.txt".to_string())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clicking_a_dirty_tab_close_button_asks_before_discarding() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("one\n");
    app.dispatch(Command::InsertText("x".to_string()))
        .expect("dirty the buffer");
    let id = app.state.active_document().expect("document").id;
    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(rect);
    let tabs = crate::ui::pane::document_tabs(&app.state, &app.state.panes[&pane_id], 4);
    let tab = tabs.iter().find(|tab| tab.id == id).expect("tab");
    click(
        &mut app,
        inner.x + tab.close_start(),
        inner.y,
        KeyModifiers::NONE,
    );

    assert!(app.state.documents.contains_key(&id));
    assert!(matches!(app.state.mode, InputMode::Prompt(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_arrows_and_wheel_scroll_an_overflowing_strip() {
    use ratatui::crossterm::event::MouseEventKind;

    let (mut app, dir) = fixture("one\n");
    for index in 0..10 {
        let path = dir.join(format!("long-document-name-{index}.txt"));
        std::fs::write(&path, "x\n").expect("file");
        app.open_document(path).expect("open");
    }
    let pane_id = app.state.focused_pane_id().expect("pane");
    let rect = app.state.ui.pane_rects[&pane_id];
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(rect);
    let tabs = crate::ui::pane::document_tabs(&app.state, &app.state.panes[&pane_id], 4);
    let total = tabs
        .last()
        .map(|tab| tab.start.saturating_add(tab.width))
        .unwrap_or(0);
    assert!(
        total > inner.width,
        "tabs should overflow: {total} > {}",
        inner.width
    );

    app.state.panes.get_mut(&pane_id).expect("pane").tab_scroll = 0;
    click(
        &mut app,
        inner.x + inner.width - 1,
        inner.y,
        ratatui::crossterm::event::KeyModifiers::NONE,
    );
    assert!(app.state.panes[&pane_id].tab_scroll > 0);

    click(
        &mut app,
        inner.x,
        inner.y,
        ratatui::crossterm::event::KeyModifiers::NONE,
    );
    assert_eq!(app.state.panes[&pane_id].tab_scroll, 0);

    wheel(&mut app, MouseEventKind::ScrollDown, inner.x + 2, inner.y);
    assert!(app.state.panes[&pane_id].tab_scroll > 0);
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
    // Room for two minimum panes (10 + separator + 19), not for three.
    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 30, 12));
    for _ in 0..6 {
        let _ = app.dispatch(Command::SplitLeftRight);
    }
    ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 30, 12));
    let panes = app.state.panes.len();
    assert!(panes >= 2, "the first split must succeed");
    assert!(
        panes <= 2,
        "a 30-column terminal cannot hold three minimum panes, got {panes}"
    );
    for rect in app.state.ui.pane_rects.values() {
        assert!(rect.width >= crate::workspace::MIN_PANE_COLS, "{rect:?}");
        assert!(rect.height >= crate::workspace::MIN_PANE_ROWS, "{rect:?}");
    }
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

    app.dispatch(Command::SelectTheme).expect("prompt");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.select_candidate("dark");
    } else {
        panic!("expected theme prompt");
    }
    app.submit_prompt().expect("dark");
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
        .map(|document| document.rope().to_string())
        .expect("document");
    app.dispatch(Command::ToggleTerminal).expect("toggle");
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let after = app
        .state
        .documents
        .values()
        .next()
        .map(|document| document.rope().to_string())
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
fn prompt_typing_keeps_the_case_the_user_typed() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::NewPrompt).expect("new file");
    for ch in ['N', 'o', 't', 'e', 's'] {
        let mods = if ch.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), mods));
    }
    assert_eq!(prompt_input(&app), "Notes");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_find_preview_copies_the_buffer_once_per_revision() {
    let (mut app, dir) = fixture("needle needle\n");
    app.preview_search("needle".to_string());
    let first = app.search_text.as_ref().expect("cache").2.clone();
    app.preview_search("needles".to_string());
    let second = app.search_text.as_ref().expect("cache").2.clone();
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "typing must reuse the same copy while the buffer is unchanged"
    );

    app.dispatch(Command::InsertText("x".to_string()))
        .expect("edit");
    app.preview_search("needle".to_string());
    let third = app.search_text.as_ref().expect("cache").2.clone();
    assert!(
        !std::sync::Arc::ptr_eq(&first, &third),
        "an edited buffer is read again"
    );
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
fn editorconfig_and_language_defaults_drive_tab_insertion() {
    let (mut app, dir) = fixture("x\n");
    std::fs::write(
        dir.join(".editorconfig"),
        "root = true\n[*.rs]\nindent_style = space\nindent_size = 2\n",
    )
    .expect("editorconfig");
    let rust = dir.join("main.rs");
    let go = dir.join("main.go");
    std::fs::write(&rust, "x\n").expect("rust");
    std::fs::write(&go, "x\n").expect("go");

    app.open_document(rust).expect("open rust");
    app.state
        .panes
        .get_mut(&app.state.focused_pane_id().expect("pane"))
        .expect("pane")
        .cursor = 0;
    app.dispatch(Command::InsertTab).expect("rust tab");
    assert!(text(&app).starts_with("  x"), "{:?}", text(&app));

    app.open_document(go).expect("open go");
    app.state
        .panes
        .get_mut(&app.state.focused_pane_id().expect("pane"))
        .expect("pane")
        .cursor = 0;
    app.dispatch(Command::InsertTab).expect("go tab");
    assert!(text(&app).starts_with("\tx"), "{:?}", text(&app));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cli_targets_open_multiple_files_as_tabs_and_jump_to_a_line() {
    let (mut app, dir) = fixture("one\n");
    let second = dir.join("second.txt");
    let third = dir.join("third.txt");
    std::fs::write(&second, "a\nb\nc\n").expect("second");
    std::fs::write(&third, "x\n").expect("third");

    app.open_targets(vec![crate::app::OpenTarget {
        path: second.clone(),
        line: Some(3),
    }])
    .expect("single target");
    let document = app.state.active_document().expect("second");
    assert_eq!(document.path.as_deref(), Some(second.as_path()));
    let cursor = app.state.focused_pane().expect("pane").cursor;
    assert_eq!(document.line_of_char(cursor), 2);

    app.open_targets(vec![
        crate::app::OpenTarget {
            path: third.clone(),
            line: None,
        },
        crate::app::OpenTarget {
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

    app.dispatch(Command::SelectTheme).expect("prompt");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        prompt.select_candidate("dark");
    } else {
        panic!("expected theme prompt");
    }
    app.submit_prompt().expect("dark");
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
