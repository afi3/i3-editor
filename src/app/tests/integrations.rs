use super::*;

#[test]
fn files_above_the_read_only_cap_are_refused() {
    let (mut app, dir) = fixture("x\n");
    let path = dir.join("huge.bin");
    let file = std::fs::File::create(&path).expect("create");
    file.set_len((256 << 20) + 1).expect("size");
    drop(file);
    let error = app.open_document(path).expect_err("refused");
    assert!(error.to_string().contains("too large"), "{error}");
    assert_eq!(app.state.documents.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancelling_the_large_file_prompt_opens_nothing() {
    let (mut app, dir) = fixture("x\n");
    let path = dir.join("huge.log");
    let file = std::fs::File::create(&path).expect("create");
    file.set_len(17 << 20).expect("size");
    drop(file);

    app.open_document(path).expect("prompt");
    app.cancel_prompt();
    assert!(app.pending_large_open.is_none());
    assert_eq!(app.state.documents.len(), 1);
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

    let deadline = Instant::now() + Duration::from_secs(15);
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
    let deadline = Instant::now() + Duration::from_secs(15);
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
    let deadline = Instant::now() + Duration::from_secs(15);
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

    let deadline = Instant::now() + Duration::from_secs(15);
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
    let deadline = Instant::now() + Duration::from_secs(15);
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
fn toggle_fold_hides_the_block_and_typing_in_the_header_keeps_it() {
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
    let document = app.state.active_document().expect("doc");
    assert_eq!(document.folded, vec![0..3]);
    assert!(document.is_hidden(1));
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
    let base = dir.join("swap");
    app.swap = Some(crate::recovery::Instance::claim(&base).expect("swap"));
    app.flush_swaps();

    // A crash drops the lock and leaves the instance directory behind.
    let crashed = app.swap.take().expect("swap");
    drop(crashed);
    let instance = crate::recovery::Instance::claim(&base).expect("swap");
    let mut documents = std::collections::HashMap::new();
    let mut ids = crate::state::Ids::default();
    let recovered = crate::recovery::recover_from(&base, instance.dir(), &mut documents, &mut ids);
    assert_eq!(recovered.len(), 1);
    let id = recovered[0];
    let document = documents.remove(&id).expect("recovered document");
    assert_eq!(document.rope().to_string(), "xone\n");
    assert!(document.dirty);
    assert_eq!(document.path.as_deref(), Some(path.as_path()));

    let swap_dir = instance.dir().to_path_buf();
    app.swap = Some(instance);
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
        std::fs::read_dir(&swap_dir)
            .expect("swap dir")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".swap"))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recovery_asks_before_overwriting_a_file_changed_after_the_swap() {
    let (mut app, dir) = fixture("one\n");
    let path = dir.join("note.txt");
    app.dispatch(Command::InsertText("x".to_string()))
        .expect("type");
    let base = dir.join("swap");
    app.swap = Some(crate::recovery::Instance::claim(&base).expect("swap"));
    app.flush_swaps();
    let crashed = app.swap.take().expect("swap");
    drop(crashed);

    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path, "newer\n").expect("external write");

    let instance = crate::recovery::Instance::claim(&base).expect("swap");
    let mut documents = std::collections::HashMap::new();
    let mut ids = crate::state::Ids::default();
    let recovered = crate::recovery::recover_from(&base, instance.dir(), &mut documents, &mut ids);
    assert_eq!(recovered.len(), 1);
    let id = recovered[0];
    let document = documents.remove(&id).expect("recovered document");
    app.swap = Some(instance);
    app.state.documents.insert(id, document);
    app.recoveries = vec![id];
    app.begin_recovery_prompt();

    app.submit_prompt().expect("save recovered buffer");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "newer\n",
        "the recovered buffer must not overwrite a newer file"
    );
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("external change prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::ExternalChange);
    assert_eq!(prompt.document, Some(id));

    app.submit_prompt().expect("overwrite");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "xone\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_all_and_quit_asks_before_overwriting_an_external_change() {
    let (mut app, dir) = fixture("x\n");
    let path = app
        .state
        .active_document()
        .and_then(|document| document.path.clone())
        .expect("path");
    app.dispatch(Command::InsertText("y".to_string()))
        .expect("type");
    std::fs::write(&path, "external\n").expect("external write");

    app.dispatch(Command::Quit).expect("quit");
    if let InputMode::Prompt(prompt) = &mut app.state.mode {
        assert_eq!(prompt.selected_candidate(), Some("Save all and quit"));
    }
    app.submit_prompt().expect("save and quit");

    assert!(
        !app.state.should_quit,
        "must not quit over an external change"
    );
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("external change prompt missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::ExternalChange);
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "external\n");

    app.submit_prompt().expect("overwrite");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "yx\n");
    assert!(!app.state.should_quit);
    assert!(!app.state.active_document().expect("document").dirty);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn accepting_a_completion_replaces_the_range_the_server_asked_for() {
    let (mut app, dir) = fixture("foo\n");
    let id = app.state.active_document_id().expect("document");
    app.state.focused_pane_mut().expect("pane").cursor = 3;
    let revision = app.state.documents[&id].revision;
    app.state.completions = Some(crate::state::CompletionPopup {
        document: id,
        revision,
        items: vec![crate::lsp::CompletionItem {
            label: "foobar".to_string(),
            insert: "foobar".to_string(),
            detail: String::new(),
            range: Some(crate::lsp::CompletionRange {
                start_line: 0,
                start_character: 0,
                end_line: 0,
                end_character: 3,
            }),
        }],
        selected: 0,
    });
    let enter = crate::input::KeyCombo {
        code: ratatui::crossterm::event::KeyCode::Enter,
        mods: ratatui::crossterm::event::KeyModifiers::NONE,
    };
    assert!(app.handle_completion_key(enter));
    assert_eq!(app.state.documents[&id].rope().to_string(), "foobar\n");
    assert_eq!(app.state.focused_pane().expect("pane").cursor, 6);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_completion_for_an_older_revision_is_discarded() {
    let (mut app, dir) = fixture("foo\n");
    let id = app.state.active_document_id().expect("document");
    let revision = app.state.documents[&id].revision;
    app.state.completions = Some(crate::state::CompletionPopup {
        document: id,
        revision: revision + 1,
        items: vec![crate::lsp::CompletionItem {
            label: "foobar".to_string(),
            insert: "foobar".to_string(),
            detail: String::new(),
            range: None,
        }],
        selected: 0,
    });
    let enter = crate::input::KeyCombo {
        code: ratatui::crossterm::event::KeyCode::Enter,
        mods: ratatui::crossterm::event::KeyModifiers::NONE,
    };
    assert!(app.handle_completion_key(enter));
    assert_eq!(app.state.documents[&id].rope().to_string(), "foo\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
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
            filenames: Vec::new(),
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

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !app.state.lsp_diagnostics.contains_key(&id) {
        app.pump_lsp();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.state.lsp_diagnostics[&id][0].message, "unknown name");

    app.dispatch(Command::LspCompletion).expect("completion");
    let deadline = Instant::now() + Duration::from_secs(15);
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
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.state.hover.is_none() {
        app.pump_lsp();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.state.hover.as_deref(), Some("u32"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.state.hover.is_none());

    app.dispatch(Command::LspDefinition).expect("definition");
    let deadline = Instant::now() + Duration::from_secs(15);
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
fn lsp_install_refuses_an_already_configured_language() {
    let (mut app, dir) = fixture("fn main() {}\n");
    let rust = dir.join("main.rs");
    std::fs::write(&rust, "fn main() {}\n").expect("rust file");
    app.open_document(rust).expect("open rust");
    app.state.config.lsp.servers.insert(
        "rust".to_string(),
        crate::config::LspServerConfig {
            command: "rust-analyzer".to_string(),
            args: Vec::new(),
            extensions: vec!["rs".to_string()],
            filenames: Vec::new(),
        },
    );
    app.dispatch(Command::LspInstall).expect("install command");
    assert!(app.lsp_install.is_none());
    assert!(
        app.state.ui.message.contains("already configured"),
        "{}",
        app.state.ui.message
    );
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
            filenames: Vec::new(),
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
fn a_commit_runs_off_the_ui_thread_and_reports_its_summary() {
    use std::process::Command as Process;
    use std::time::{Duration, Instant};

    if Process::new("git").arg("--version").output().is_err() {
        return;
    }
    let (mut app, dir) = fixture("one\n");
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
    ] {
        assert!(
            Process::new("git")
                .args(&args)
                .current_dir(&dir)
                .status()
                .expect("git")
                .success(),
            "{args:?}"
        );
    }
    assert!(
        Process::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .expect("git")
            .success()
    );
    app.submit_commit("first").expect("commit");
    assert!(
        app.git_command.is_some(),
        "the commit must not run on the UI thread"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.git_command.is_some() && Instant::now() < deadline {
        app.pump_git_command();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.git_command.is_none(), "commit did not finish");
    assert!(
        app.state.ui.message.contains("first"),
        "{}",
        app.state.ui.message
    );
    let status = Process::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&dir)
        .output()
        .expect("status");
    assert!(
        status.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
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
    let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < end && !app.state.git_marks.contains_key(&id) {
        app.pump_git_marks();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.git_command.is_some() && std::time::Instant::now() < deadline {
        app.pump_git_command();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.git_command.is_none(), "commit did not finish");
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
fn a_wrapped_line_taller_than_the_pane_scrolls_by_row() {
    use crate::command::Move;

    let source = "word ".repeat(200);
    let (mut app, dir) = fixture(&source);
    app.state.config.editor.word_wrap = true;
    crate::ui::compute_layout(&mut app.state, ratatui::layout::Rect::new(0, 0, 40, 12));
    for _ in 0..40 {
        app.dispatch(Command::MoveCursor(Move::Down)).expect("down");
    }
    let pane_id = app.state.focused_pane_id().expect("pane");
    let cursor = app.state.panes[&pane_id].cursor;
    let scroll_top = app.state.panes[&pane_id].scroll_top;
    let scroll_row = app.state.panes[&pane_id].scroll_row;
    let document = app.state.active_document().expect("doc");
    assert_eq!(document.line_of_char(cursor), 0, "still one line");
    assert_eq!(scroll_top, 0, "the offset is inside the line");
    assert!(
        scroll_row > 0,
        "the view must follow the caret down the wrapped line"
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
        assert!(document.rope().to_string().contains("\r\n"));
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
fn wheel_over_read_only_overlays_scrolls_them_not_the_document() {
    use ratatui::crossterm::event::MouseEventKind;

    let text = (0..60).map(|n| format!("line {n}\n")).collect::<String>();
    let (mut app, dir) = fixture(&text);
    let pane_id = app.state.focused_pane_id().expect("pane");
    let column = 40;
    let row = 12;

    app.dispatch(Command::Help).expect("help");
    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.help.as_ref().expect("help").scroll, 3);
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);

    app.state.help = None;
    app.state.git_diff = Some(crate::state::DiffView {
        title: " diff ".to_string(),
        lines: (0..60).map(|n| format!("line {n}")).collect(),
        scroll: 0,
    });
    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.git_diff.as_ref().expect("diff").scroll, 3);
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);

    app.state.git_diff = None;
    app.state.mcp_view = Some(crate::state::HelpView {
        lines: (0..60).map(|n| format!("line {n}")).collect(),
        scroll: 0,
    });
    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.mcp_view.as_ref().expect("mcp").scroll, 3);
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);

    app.state.mcp_view = None;
    app.state.hunks = Some(crate::state::HunkPicker {
        title: "hunk".to_string(),
        relative: "note.txt".to_string(),
        stage: true,
        hunks: vec![crate::git::Hunk {
            header: "@@ -1 +1 @@".to_string(),
            lines: (0..20).map(|n| format!("line {n}")).collect(),
            patch: String::new(),
        }],
        selected: 0,
        scroll: 0,
    });
    wheel(&mut app, MouseEventKind::ScrollDown, column, row);
    assert_eq!(app.state.hunks.as_ref().expect("hunks").scroll, 3);
    assert_eq!(app.state.panes[&pane_id].scroll_top, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn click_on_a_read_only_overlay_does_not_move_the_cursor() {
    use ratatui::crossterm::event::KeyModifiers;

    let (mut app, dir) = fixture("alpha\nbeta\n");
    app.dispatch(Command::Help).expect("help");
    let before = cursor(&app);
    click(&mut app, 40, 12, KeyModifiers::NONE);
    assert_eq!(cursor(&app), before);
    assert!(app.state.help.is_some());
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

#[test]
fn closing_the_mcp_resource_view_releases_its_subscription() {
    let (mut app, dir) = fixture("x\n");
    app.mcp_subscriptions
        .insert(("mock".to_string(), "mock://readme".to_string()));
    app.mcp_resource_view = Some(("mock".to_string(), "mock://readme".to_string()));

    app.close_mcp_overlays();
    assert!(app.state.mcp_view.is_none());
    assert!(app.mcp_resource_view.is_none());
    assert!(app.mcp_subscriptions.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn different_roots_get_different_session_files() {
    let first =
        crate::session::path_for(std::path::Path::new("/tmp/i3-session-alpha")).expect("path");
    let second =
        crate::session::path_for(std::path::Path::new("/tmp/i3-session-beta")).expect("path");
    assert_ne!(first, second);
    assert_eq!(
        Some(first.clone()),
        crate::session::path_for(std::path::Path::new("/tmp/i3-session-alpha"))
    );
    let name = first
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("session-") && name.ends_with(".toml"),
        "{name}"
    );
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
