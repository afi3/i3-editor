use super::*;

#[test]
fn the_provider_picker_selects_and_persists_a_provider() {
    let (mut app, dir) = fixture("x\n");
    {
        let ai = &mut app.state.config.ai;
        ai.providers.insert(
            "alpha".to_string(),
            crate::config::ProviderConfig {
                base_url: "https://alpha.test/v1".to_string(),
                model: "alpha-model".to_string(),
                ..Default::default()
            },
        );
        ai.providers.insert(
            "beta".to_string(),
            crate::config::ProviderConfig {
                base_url: "https://beta.test/v1".to_string(),
                model: "beta-model".to_string(),
                ..Default::default()
            },
        );
        assert!(ai.activate("alpha"));
    }
    assert_eq!(app.state.config.ai.model, "alpha-model");
    app.state.config_path = Some(dir.join("config.toml"));

    app.dispatch(Command::SelectAiProvider)
        .expect("open picker");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("provider prompt missing")
    };
    assert_eq!(
        prompt.candidates,
        vec!["alpha".to_string(), "beta".to_string()],
        "the picker lists the registry"
    );
    prompt.selected = 1;
    app.submit_prompt().expect("select provider");
    assert_eq!(app.state.config.ai.active_provider, "beta");
    assert_eq!(app.state.config.ai.base_url, "https://beta.test/v1");
    assert_eq!(app.state.config.ai.model, "beta-model");
    assert!(
        app.state.ui.message.contains("beta-model"),
        "{}",
        app.state.ui.message
    );
    let saved = std::fs::read_to_string(dir.join("config.toml")).expect("config file");
    assert!(
        saved.contains("active_provider = \"beta\""),
        "switch must be persisted: {saved}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_provider_picker_without_a_registry_reports_it() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::SelectAiProvider)
        .expect("no registry");
    assert!(
        app.state.ui.message.contains("ai.providers"),
        "{}",
        app.state.ui.message
    );
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
    app.apply_document_indents();
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
        vec![crate::app::OpenTarget {
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
    assert_eq!(crate::app::startup_message(&[]), "");
    assert_eq!(crate::app::startup_message(&["only".to_string()]), "only");
    assert_eq!(
        crate::app::startup_message(&["first".to_string(), "second".to_string()]),
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
            .any(|name| name == "select_workspace")
    );
    assert!(
        !prompt
            .candidates
            .iter()
            .any(|name| name.starts_with("switch_workspace_"))
    );
    app.dispatch(Command::SelectWorkspace)
        .expect("workspace prompt");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("workspace prompt missing")
    };
    let first = prompt
        .candidates
        .iter()
        .find(|candidate| candidate.starts_with("1  "))
        .cloned()
        .expect("first workspace");
    prompt.select_candidate(&first);
    app.submit_prompt().expect("switch");
    assert_eq!(app.state.workspaces.active, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_commands_open_and_toggle_the_panel() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("ai_chat");
    assert!(app.state.ai.visible);
    assert!(app.state.ai.focused);
    assert!(!app.state.ai.entries.is_empty());

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.state.ai.focused);

    app.dispatch(Command::AiPanel).expect("ai_panel");
    assert!(!app.state.ai.visible);
    app.dispatch(Command::AiPanel).expect("ai_panel");
    assert!(app.state.ai.visible);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn embeddings_come_back_in_input_order() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    if std::process::Command::new("curl")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping embeddings: curl unavailable");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0u8; 8192];
        let _ = stream.read(&mut request);
        // Deliberately reversed: the transport must sort by `index`.
        let body = r#"{"data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let mut config = crate::ai::test_config(format!("http://{address}/v1"));
    config.api_key = "test-key".to_string();
    let vectors =
        crate::ai::embed(&config, "mock-embed", &["a".into(), "b".into()]).expect("embeddings");
    assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
}

#[test]
fn the_native_search_tool_needs_no_path_argument() {
    let (mut app, dir) = fixture("needle in the haystack\n");
    let call = crate::state::AiToolCall {
        id: "c1".to_string(),
        server: crate::native::SERVER.to_string(),
        tool: "search".to_string(),
        capability: "read".to_string(),
        arguments: serde_json::json!({"query": "needle"}),
    };
    let root = app.state.explorer.root.clone();
    let output = app.run_native_tool(&root, &call).expect("search runs");
    assert!(output.contains("needle"), "{output}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_saved_conversation_with_a_dangling_tool_call_is_repaired() {
    let mut conversation = vec![
        serde_json::json!({"role": "user", "content": "hi"}),
        serde_json::json!({"role": "assistant", "content": null, "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "x", "arguments": "{}"}}
        ]}),
        serde_json::json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
        serde_json::json!({"role": "assistant", "content": null, "tool_calls": [
            {"id": "c2", "type": "function", "function": {"name": "y", "arguments": "{}"}}
        ]}),
    ];
    crate::app::App::repair_dangling_tool_calls(&mut conversation);
    let last = conversation.last().expect("tool message");
    assert_eq!(last["role"], "tool");
    assert_eq!(last["tool_call_id"], "c2");
    assert_eq!(
        conversation
            .iter()
            .filter(|message| message["tool_call_id"] == "c1")
            .count(),
        1,
        "an answered call is not closed twice"
    );
}

#[test]
fn the_subscription_picker_releases_servers_with_spaces_in_their_name() {
    let (mut app, dir) = fixture("x\n");
    app.mcp_subscriptions
        .insert(("my server".to_string(), "mock://docs".to_string()));
    let mut prompt =
        crate::input::prompt::PromptState::new(crate::input::PromptKind::McpSubscriptions);
    app.fill_subscription_candidates(&mut prompt);
    assert_eq!(prompt.candidates, vec!["my server mock://docs"]);
    app.unsubscribe_mcp_subscription(0);
    assert!(app.mcp_subscriptions.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_mcp_tool_call_is_answered_in_the_conversation() {
    let (mut app, dir) = fixture("x\n");
    app.state.ai.conversation = vec![serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{"id": "c1", "type": "function",
                        "function": {"name": "mcp__srv__tool", "arguments": "{}"}}]
    })];
    app.state.ai.waiting = Some(crate::state::AiWaiting {
        id: "c1".to_string(),
        server: "srv".to_string(),
        tool: "tool".to_string(),
        capability: "read".to_string(),
    });
    app.state.ai.busy = true;
    app.handle_mcp_failure("srv", "request timed out".to_string());

    let last = app.state.ai.conversation.last().expect("tool message");
    assert_eq!(last["role"], "tool");
    assert_eq!(last["tool_call_id"], "c1");
    assert!(
        last["content"]
            .as_str()
            .is_some_and(|text| text.contains("timed out"))
    );
    assert!(!app.state.ai.busy, "the failed turn must stop waiting");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_mcp_notice_does_not_abort_the_running_call() {
    let (mut app, dir) = fixture("x\n");
    app.state.ai.waiting = Some(crate::state::AiWaiting {
        id: "c1".to_string(),
        server: "srv".to_string(),
        tool: "tool".to_string(),
        capability: "read".to_string(),
    });
    app.state.ai.queued_question = Some("next question".to_string());
    app.handle_mcp_notice("MCP srv: tool list changed; refresh tools".to_string());
    assert!(app.state.ai.waiting.is_some(), "a notice is not a failure");
    assert!(app.state.ai.queued_question.is_some());
    assert_eq!(
        app.state.ui.message, "MCP srv: tool list changed; refresh tools",
        "the user must be told the tool list changed"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_queued_question_is_resumed_when_a_server_failure_ends_tool_loading() {
    let (mut app, dir) = fixture("x\n");
    app.state.ai.queued_question = Some("what now?".to_string());
    app.handle_mcp_failure("srv", "server stopped".to_string());
    assert!(
        app.state.ai.queued_question.is_none(),
        "the question must not be stranded"
    );
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["role"] == "user" && message["content"] == "what now?"),
        "{:?}",
        app.state.ai.conversation
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failure_on_another_server_leaves_the_waiting_call_alone() {
    let (mut app, dir) = fixture("x\n");
    app.state.ai.waiting = Some(crate::state::AiWaiting {
        id: "c1".to_string(),
        server: "srv".to_string(),
        tool: "tool".to_string(),
        capability: "read".to_string(),
    });
    app.handle_mcp_failure("other", "server stopped".to_string());
    assert!(app.state.ai.waiting.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_rules_overlay_takes_keys_before_the_ai_panel_under_it() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.focused = true;
    app.dispatch(Command::AiRules).expect("rules");
    assert!(app.state.rules_view.is_some(), "overlay is open");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        app.state.rules_view.is_none(),
        "Escape must close the overlay on screen, not the panel behind it"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn blocked_calls_and_model_requests_reach_the_log_file() {
    let (app, dir) = fixture("x\n");
    let log = dir.join("log.jsonl");
    let _log = crate::logging::use_path_for_test(Some(log.clone()));

    crate::app::audit::blocked(
        &app.state,
        "call-9",
        "native",
        "write_file",
        &serde_json::json!({"path": "new.txt"}),
        "tool policy",
    );
    crate::app::audit::model_request(
        &app.state,
        &crate::app::audit::ModelRequest {
            round: 2,
            model: "mock-model",
            original_messages: 4,
            sent_messages: 3,
            estimated_tokens: 120,
            latency: std::time::Duration::from_millis(7),
            stop: "tool_calls",
            prompt_tokens: Some(100),
            completion_tokens: Some(20),
            tool_calls: 1,
        },
    );

    let text = std::fs::read_to_string(&log).expect("log file");
    let events: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        events.iter().any(|event| {
            event["event"] == "tool_blocked"
                && event["tool"] == "write_file"
                && event["reason"] == "tool policy"
                && event.get("arguments").is_none()
        }),
        "arguments must stay out of the file: {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            event["event"] == "model_request"
                && event["model"] == "mock-model"
                && event["round"] == 2
                && event["latency_ms"] == 7
        }),
        "{events:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn watcher_faults_are_logged() {
    let (mut app, dir) = fixture("x\n");
    let log = dir.join("log.jsonl");
    let _log = crate::logging::use_path_for_test(Some(log.clone()));

    let workspace = app.state.workspaces.active().id;
    let watcher = crate::watch::Watcher::empty().expect("watcher");
    watcher.inject_for_test(Ok(
        notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan)
    ));
    watcher.inject_for_test(Err(notify::Error::generic("boom")));
    app.watchers.push((workspace, watcher));

    app.pump_watcher();

    let text = std::fs::read_to_string(&log).expect("log file");
    assert!(text.contains("\"event\":\"watcher_rescan\""), "{text}");
    assert!(text.contains("\"event\":\"watcher_error\""), "{text}");
    assert!(text.contains("boom"), "{text}");
    assert!(
        app.state.ui.message.contains("overflow") && app.state.ui.message.contains("boom"),
        "the error must survive the rescan notice: {}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn semantic_index_and_status_report_the_built_index() {
    let _guard = crate::semantic::lock_for_test();
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0u8; 16384];
        let _ = stream.read(&mut request);
        let body = r#"{"data":[{"index":0,"embedding":[1,0]}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.embedding_model = "mock-embed".to_string();
    crate::semantic::use_path_for_test(Some(dir.join("semantic_index.json")));
    let _log = crate::logging::use_path_for_test(Some(dir.join("log.jsonl")));
    // A blank buffer has no chunks, and a dirty one is skipped: neither may
    // inflate the summary.
    let blank = dir.join("blank.txt");
    std::fs::write(&blank, "\n\n\n").expect("blank");
    let blank_id = app.state.ids.document();
    app.state.documents.insert(
        blank_id,
        crate::editor::document::Document::open(blank_id, blank).expect("open blank"),
    );
    app.state
        .documents
        .get_mut(&blank_id)
        .expect("blank")
        .insert(0, "x", 0, (0, 0));
    app.dispatch(Command::AiSemanticIndex).expect("index");
    assert!(
        app.semantic_index_task.is_some(),
        "the build must run in the background"
    );
    assert!(
        app.state.ui.message.contains("building in the background"),
        "{}",
        app.state.ui.message
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline && app.semantic_index_task.is_some() {
        app.pump_semantic();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.semantic_index_task.is_none(), "build did not finish");
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("dirty buffer(s) skipped")),
        "{:?}",
        app.state.ai.entries
    );
    assert!(
        app.state.ui.message.contains("from 1 file(s)"),
        "a skipped buffer must not be counted: {}",
        app.state.ui.message
    );
    app.dispatch(Command::AiSemanticStatus).expect("status");
    assert!(
        app.state.ui.message.contains("semantic index: 1 chunks"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unchanged_file_is_embedded_only_once() {
    let _guard = crate::semantic::lock_for_test();
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    // Exactly one embedding request is served; a second one would be refused,
    // which is how the test proves the rebuild skipped the file.
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0u8; 16384];
        let _ = stream.read(&mut request);
        let body = r#"{"data":[{"index":0,"embedding":[1,0]}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.embedding_model = "mock-embed".to_string();
    let index_path = dir.join("semantic_index.json");
    crate::semantic::use_path_for_test(Some(index_path.clone()));

    let build = |app: &mut App| {
        app.dispatch(Command::AiSemanticIndex).expect("build");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline && app.semantic_index_task.is_some() {
            app.pump_semantic();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.semantic_index_task.is_none(), "build did not finish");
    };
    build(&mut app);
    assert!(
        app.state.ui.message.contains("1 embedded"),
        "{}",
        app.state.ui.message
    );

    assert!(
        crate::semantic::Index::load(&index_path).is_some(),
        "the first build must have written the index"
    );
    build(&mut app);
    assert!(
        app.state.ui.message.contains("1 reused"),
        "{}",
        app.state.ui.message
    );
    assert!(
        !app.state.ui.message.contains("failed"),
        "the rebuild must not embed again: {}",
        app.state.ui.message
    );
    let index = crate::semantic::Index::load(&index_path).expect("index");
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.files.len(), 1);

    crate::semantic::use_path_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn semantic_memory_attaches_the_closest_chunk() {
    let _guard = crate::semantic::lock_for_test();
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for body in [
            r#"{"data":[{"index":0,"embedding":[1,0]}]}"#,
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
        ] {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 16384];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.embedding_model = "mock-embed".to_string();
    app.state.config.ai.semantic_memory = true;
    app.state.config.ai.semantic_top_k = 1;
    let index_path = dir.join("semantic_index.json");
    crate::semantic::use_path_for_test(Some(index_path.clone()));
    let mut index = crate::semantic::Index::empty("mock-embed");
    index.entries.push(crate::semantic::Entry {
        chunk: crate::semantic::Chunk {
            path: dir.join("note.txt"),
            start_line: 0,
            end_line: 1,
            text: "the needle".to_string(),
        },
        vector: vec![1.0, 0.0],
    });
    index.save(&index_path).expect("seed");

    app.dispatch(Command::AiChat).expect("panel");
    app.state.ai.input = "where is the needle".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.pump_semantic();
        app.pump_ai();
        if app
            .state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("(semantic)"))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("(semantic)")),
        "{:?}",
        app.state.ai.entries
    );

    crate::semantic::use_path_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn semantic_query_embedding_does_not_block_the_editor() {
    let _guard = crate::semantic::lock_for_test();
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for (body, delay) in [
            (r#"{"data":[{"index":0,"embedding":[1,0]}]}"#, 300u64),
            (
                r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
                0u64,
            ),
        ] {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 16384];
            let _ = stream.read(&mut request);
            std::thread::sleep(Duration::from_millis(delay));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.embedding_model = "mock-embed".to_string();
    app.state.config.ai.semantic_memory = true;
    app.state.config.ai.semantic_top_k = 1;
    let index_path = dir.join("semantic_index.json");
    crate::semantic::use_path_for_test(Some(index_path.clone()));
    let mut index = crate::semantic::Index::empty("mock-embed");
    index.entries.push(crate::semantic::Entry {
        chunk: crate::semantic::Chunk {
            path: dir.join("note.txt"),
            start_line: 0,
            end_line: 1,
            text: "the needle".to_string(),
        },
        vector: vec![1.0, 0.0],
    });
    index.save(&index_path).expect("seed");

    app.dispatch(Command::AiChat).expect("panel");
    app.state.ai.input = "where is the needle".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));

    assert!(
        app.semantic_task.is_some(),
        "the question must be deferred to the worker"
    );
    assert!(
        !app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["role"] == "user"),
        "the request must not start before the embedding arrives: {:?}",
        app.state.ai.conversation
    );

    // A second Enter while the worker runs must not start a competing request.
    app.state.ai.input = "second question".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(
        app.state
            .ui
            .message
            .contains("still preparing semantic context"),
        "{}",
        app.state.ui.message
    );
    assert!(
        app.semantic_task.is_some(),
        "the first question keeps its worker"
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.pump_semantic();
        app.pump_ai();
        if app
            .state
            .ai
            .conversation
            .iter()
            .any(|message| message.to_string().contains("(semantic)"))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| message.to_string().contains("(semantic)")),
        "{:?}",
        app.state.ai.conversation
    );
    assert!(app.semantic_task.is_none(), "the task must be consumed");

    crate::semantic::use_path_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn embeddings_retry_a_transient_failure() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for (status, body) in [
            ("500 Internal Server Error", ""),
            ("200 OK", r#"{"data":[{"index":0,"embedding":[1,0]}]}"#),
        ] {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let mut config = crate::ai::test_config(format!("http://{address}/v1"));
    config.api_key = "test-key".to_string();
    config.retry_attempts = 1;
    config.retry_base_ms = 10;
    let vectors =
        crate::ai::embed(&config, "mock-embed", &["x".to_string()]).expect("retry succeeds");
    assert_eq!(vectors, vec![vec![1.0, 0.0]]);
}

#[test]
fn a_failed_batch_keeps_the_index_built_so_far() {
    let _guard = crate::semantic::lock_for_test();
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let data: Vec<serde_json::Value> = (0..16)
        .map(|index| serde_json::json!({"index": index, "embedding": [1.0, 0.0]}))
        .collect();
    let first_body = serde_json::json!({"data": data}).to_string();
    std::thread::spawn(move || {
        for (status, body) in [
            ("200 OK", first_body.as_str()),
            ("500 Internal Server Error", ""),
        ] {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 32768];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let text: String = (0..700).map(|line| format!("line {line}\n")).collect();
    let (mut app, dir) = fixture(&text);
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.embedding_model = "mock-embed".to_string();
    app.state.config.ai.retry_attempts = 0;
    let index_path = dir.join("semantic_index.json");
    crate::semantic::use_path_for_test(Some(index_path.clone()));

    app.dispatch(Command::AiSemanticIndex).expect("index");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline && app.semantic_index_task.is_some() {
        app.pump_semantic();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.semantic_index_task.is_none(), "build did not finish");
    assert!(
        app.state.ui.message.contains("2 chunk(s) failed"),
        "the failed batch must be reported: {}",
        app.state.ui.message
    );
    let index = crate::semantic::Index::load(&index_path).expect("index");
    assert_eq!(index.entries.len(), 16, "the successful batch is kept");

    crate::semantic::use_path_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn oversized_chunks_are_reported() {
    let _guard = crate::semantic::lock_for_test();
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0u8; 8192];
        let _ = stream.read(&mut request);
        let body = r#"{"data":[{"index":0,"embedding":[1,0]}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    // 40 short lines fill the first chunk; the 600-char line is its own chunk and
    // cannot fit the 500-char budget, so it must be counted, not dropped silently.
    let mut text = String::new();
    for line in 0..40 {
        text.push_str(&format!("line {line}\n"));
    }
    text.push_str(&"x".repeat(600));
    text.push('\n');

    let (mut app, dir) = fixture(&text);
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.embedding_model = "mock-embed".to_string();
    app.state.config.ai.semantic_max_chars = 500;
    let index_path = dir.join("semantic_index.json");
    crate::semantic::use_path_for_test(Some(index_path.clone()));

    app.dispatch(Command::AiSemanticIndex).expect("index");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline && app.semantic_index_task.is_some() {
        app.pump_semantic();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.semantic_index_task.is_none(), "build did not finish");
    assert!(
        app.state.ui.message.contains("1 chunk(s) too large"),
        "{}",
        app.state.ui.message
    );
    let index = crate::semantic::Index::load(&index_path).expect("index");
    assert_eq!(index.entries.len(), 1);

    crate::semantic::use_path_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_live_request_is_not_replaced() {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    std::thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_millis(900);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    seen.fetch_add(1, Ordering::Relaxed);
                    let mut request = [0u8; 8192];
                    let _ = stream.read(&mut request);
                    // Hold the connection so the client task stays alive.
                    std::thread::sleep(Duration::from_millis(300));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    });

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.api_key = "test-key".to_string();
    app.state.config.ai.retry_attempts = 0;

    app.finish_ai_question("first".to_string(), Vec::new());
    assert!(app.ai_task.is_some(), "the first request must be live");
    app.finish_ai_question("second".to_string(), Vec::new());

    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        connections.load(Ordering::Relaxed),
        1,
        "a live request must not be replaced by a second one"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_subscription_picker_releases_one_entry() {
    let (mut app, dir) = fixture("x\n");
    app.mcp_subscriptions
        .insert(("mock".to_string(), "mock://readme".to_string()));
    app.mcp_subscriptions
        .insert(("mock".to_string(), "mock://notes".to_string()));

    app.dispatch(Command::McpUnsubscribe).expect("open picker");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("subscription prompt missing")
    };
    assert_eq!(prompt.candidates.len(), 2, "{:?}", prompt.candidates);
    prompt.selected = prompt
        .candidates
        .iter()
        .position(|candidate| candidate == "mock mock://notes")
        .expect("notes candidate");
    app.submit_prompt().expect("release one");

    assert_eq!(app.mcp_subscriptions.len(), 1);
    assert!(
        app.mcp_subscriptions
            .contains(&("mock".to_string(), "mock://readme".to_string()))
    );
    assert!(
        app.state.ui.message.contains("mock://notes"),
        "{}",
        app.state.ui.message
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_verify_after_an_edit_reaches_the_next_request() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let responses = [
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"native__write_file","arguments":"{\"path\":\"new.txt\",\"content\":\"hi\\n\"}"}}]}}]}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":"final answer"}}]}"#,
    ];
    std::thread::spawn(move || {
        for body in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 65536];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.state.config.ai.verify_after_edit = true;
    app.state.config.ai.verify_after_edit_command = "t".to_string();
    app.state.config.ai.verify.insert(
        "t".to_string(),
        crate::config::VerifyConfig {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "echo broken; exit 1".to_string()],
            timeout_secs: 5,
            sandbox: false,
        },
    );

    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.input = "write the file".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.state.ai.pending.is_empty() {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    let call = app
        .state
        .ai
        .pending
        .front()
        .cloned()
        .expect("pending write");
    assert_eq!(call.server, "native");
    assert_eq!(call.tool, "write_file");

    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let verify_output = |app: &App| {
        app.state.ai.conversation.iter().any(|message| {
            message
                .get("content")
                .and_then(|value| value.as_str())
                .is_some_and(|text| text.contains("broken"))
        })
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !verify_output(&app) {
        app.pump_native_verify();
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("[verify] t after edit")),
        "the automatic pass never started: {:?}",
        app.state.ai.entries
    );
    assert!(
        verify_output(&app),
        "the failure did not reach the model: {:?}",
        app.state.ai.conversation
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("new.txt")).expect("written file"),
        "hi\n"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_harness_confirms_a_tool_and_returns_the_final_answer() {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping AI harness: python3 unavailable");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let responses = [
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"mock__echo","arguments":"{\"text\":\"hello\"}"}}]}}]}"#,
        r#"{"choices":[{"message":{"role":"assistant","content":"final answer"}}]}"#,
    ];
    std::thread::spawn(move || {
        for body in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let (mut app, dir) = fixture("x\n");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp.py");
    let mcp = crate::mcp::McpServerConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        cwd: None,
        env: BTreeMap::from([("MOCK_MCP_MODE".to_string(), "normal".to_string())]),
        url: None,
        allow_sampling: false,
        auth_token_env: None,
        headers: BTreeMap::new(),
    };
    app.state
        .config
        .mcp
        .servers
        .insert("mock".to_string(), mcp.clone());
    app.mcp = Some(crate::mcp::McpManager::new(
        BTreeMap::from([("mock".to_string(), mcp)]),
        dir.clone(),
    ));
    app.state.config.ai = crate::ai::test_config(format!("http://{address}/v1"));
    app.mcp
        .as_mut()
        .expect("mcp")
        .list_tools("mock")
        .expect("load tools");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.pump_mcp();
        if app
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.tools("mock"))
            .is_some_and(|tools| !tools.is_empty())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.mcp
            .as_ref()
            .and_then(|mcp| mcp.tools("mock"))
            .is_some_and(|tools| !tools.is_empty()),
        "mock MCP tools did not load"
    );

    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.input = "use echo".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.state.ai.pending.is_empty() {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    let call = app.state.ai.pending.front().cloned().expect("pending tool");
    assert_eq!(call.server, "mock");
    assert_eq!(call.tool, "echo");

    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline
        && !app
            .state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("final answer"))
    {
        app.pump_mcp();
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("final answer")),
        "{:?}",
        app.state.ai.entries
    );
    assert_eq!(text(&app), "x\n", "AI harness must not edit the buffer");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_waits_for_mcp_tools_before_the_first_request() {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping AI tool readiness: python3 unavailable");
        return;
    }
    let (mut app, dir) = fixture("x\n");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp.py");
    let mcp = crate::mcp::McpServerConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        cwd: None,
        env: BTreeMap::from([("MOCK_MCP_MODE".to_string(), "normal".to_string())]),
        url: None,
        allow_sampling: false,
        auth_token_env: None,
        headers: BTreeMap::new(),
    };
    app.state
        .config
        .mcp
        .servers
        .insert("mock".to_string(), mcp.clone());
    app.mcp = Some(crate::mcp::McpManager::new(
        BTreeMap::from([("mock".to_string(), mcp)]),
        dir.clone(),
    ));
    let hash = app
        .mcp
        .as_ref()
        .and_then(|mcp| mcp.config_hash("mock"))
        .expect("config hash");
    app.mcp_trusted.insert(hash);
    app.state.config.ai.base_url.clear();

    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.input = "hello".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(app.ai_task.is_none(), "request must wait for tools");
    assert!(app.state.ai.queued_question.is_some());
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("loading"))
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.state.ai.queued_question.is_some() {
        app.pump_mcp();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.state.ai.queued_question.is_none());
    assert!(
        app.state.ai.tool_map.contains_key("mock__echo"),
        "first request must carry the loaded MCP tools"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.ai_task.is_some() {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["role"] == "user" && message["content"] == "hello")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancelling_an_ai_tool_call_stops_the_mcp_server() {
    use crate::state::AiWaiting;
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping AI cancellation: python3 unavailable");
        return;
    }
    let (mut app, dir) = fixture("x\n");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp.py");
    let mcp = crate::mcp::McpServerConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        cwd: None,
        env: BTreeMap::from([("MOCK_MCP_MODE".to_string(), "normal".to_string())]),
        url: None,
        allow_sampling: false,
        auth_token_env: None,
        headers: BTreeMap::new(),
    };
    app.state
        .config
        .mcp
        .servers
        .insert("mock".to_string(), mcp.clone());
    app.mcp = Some(crate::mcp::McpManager::new(
        BTreeMap::from([("mock".to_string(), mcp)]),
        dir.clone(),
    ));
    app.mcp
        .as_mut()
        .expect("mcp")
        .list_tools("mock")
        .expect("load tools");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.pump_mcp();
        if app
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.tools("mock"))
            .is_some_and(|tools| !tools.is_empty())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.mcp.as_ref().is_some_and(|mcp| mcp.is_running("mock")));

    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.waiting = Some(AiWaiting {
        id: "call-cancel".to_string(),
        server: "mock".to_string(),
        tool: "echo".to_string(),
        capability: "destructive".to_string(),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Esc,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(!app.mcp.as_ref().is_some_and(|mcp| mcp.is_running("mock")));
    assert!(app.state.ai.waiting.is_none());
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["content"] == "Tool call cancelled.")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn native_ai_tools_read_and_edit_the_open_document() {
    use crate::state::AiToolCall;

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai.base_url.clear();
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.pending.push_back(AiToolCall {
        id: "native-read".to_string(),
        server: "native".to_string(),
        tool: "read_file".to_string(),
        capability: "read-only".to_string(),
        arguments: serde_json::json!({"path": "note.txt"}),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let tool_message = app
        .state
        .ai
        .conversation
        .last()
        .and_then(|message| message["content"].as_str())
        .expect("native read result");
    assert!(tool_message.contains("snapshot:"), "{tool_message}");
    assert!(tool_message.contains("1:"), "{tool_message}");
    app.pump_ai();

    let read = crate::native::read_file(
        &app.state.explorer.root,
        &serde_json::json!({"path": "note.txt"}),
    )
    .expect("read");
    let snapshot = read
        .lines()
        .find_map(|line| line.strip_prefix("snapshot: "))
        .expect("snapshot");
    let line = read.lines().find(|line| line.contains("|x")).expect("line");
    let (head, _) = line.split_once('|').expect("separator");
    let (number, hash) = head.split_once(':').expect("hash");
    app.state.ai.pending.push_back(AiToolCall {
        id: "native-edit".to_string(),
        server: "native".to_string(),
        tool: "edit_file".to_string(),
        capability: "destructive".to_string(),
        arguments: serde_json::json!({
            "path": "note.txt",
            "snapshot": snapshot,
            "edits": [{"line": number.parse::<usize>().unwrap(), "hash": hash, "text": "y"}]
        }),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("note.txt")).expect("edited file"),
        "y\n"
    );
    assert!(app.state.documents.values().any(|document| {
        document.rope().len_chars() == 2
            && document.rope().char(0) == 'y'
            && document.rope().char(1) == '\n'
    }));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn native_ai_write_file_creates_and_undoes_a_file() {
    use crate::state::AiToolCall;

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai.base_url.clear();
    app.state.config.ai.stream = false;
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.pending.push_back(AiToolCall {
        id: "native-write".to_string(),
        server: "native".to_string(),
        tool: "write_file".to_string(),
        capability: "destructive".to_string(),
        arguments: serde_json::json!({"path": "new.txt", "content": "new\n"}),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("new.txt")).expect("created file"),
        "new\n"
    );
    app.dispatch(Command::AiUndoEdit).expect("undo write");
    assert!(!dir.join("new.txt").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn native_edit_refuses_a_dirty_buffer() {
    use crate::state::AiToolCall;

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai.base_url.clear();
    app.dispatch(Command::AiChat).expect("ai panel");
    let read = crate::native::read_file(
        &app.state.explorer.root,
        &serde_json::json!({"path": "note.txt"}),
    )
    .expect("read");
    let snapshot = read
        .lines()
        .find_map(|line| line.strip_prefix("snapshot: "))
        .expect("snapshot");
    let line = read.lines().find(|line| line.contains("|x")).expect("line");
    let (head, _) = line.split_once('|').expect("separator");
    let (number, hash) = head.split_once(':').expect("hash");
    let document_id = *app.state.documents.keys().next().expect("document");
    app.state
        .documents
        .get_mut(&document_id)
        .expect("document")
        .dirty = true;
    app.state.ai.pending.push_back(AiToolCall {
        id: "native-dirty".to_string(),
        server: "native".to_string(),
        tool: "edit_file".to_string(),
        capability: "destructive".to_string(),
        arguments: serde_json::json!({
            "path": "note.txt",
            "snapshot": snapshot,
            "edits": [{"line": number.parse::<usize>().unwrap(), "hash": hash, "text": "y"}]
        }),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("note.txt")).expect("original file"),
        "x\n"
    );
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("unsaved"))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_file_and_selection_context_are_injected_once() {
    let (mut app, dir) = fixture("x\n");
    app.state.config.ai.base_url.clear();
    app.state.config.ai.stream = false;
    app.dispatch(Command::AiAttachFile).expect("attach file");
    assert_eq!(app.state.ai.attachments.len(), 1);
    let pane = app.state.focused_pane_mut().expect("pane");
    pane.anchor = Some(0);
    pane.cursor = 1;
    app.dispatch(Command::AiAttachSelection)
        .expect("attach selection");
    assert_eq!(app.state.ai.attachments.len(), 2);
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.input = "what is this".to_string();
    app.state.ai.cursor = app.state.ai.input.chars().count();
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let message = app
        .state
        .ai
        .conversation
        .iter()
        .find(|message| message["role"] == "user")
        .expect("user message");
    let content = message["content"].as_str().expect("content");
    assert!(content.contains("<context path="), "{content}");
    assert!(content.contains("what is this"), "{content}");
    assert!(app.state.ai.attachments.is_empty());
    if let Some(task) = app.ai_task.as_ref() {
        task.cancel();
    }
    app.ai_task = None;
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_truncated_stream_marks_the_text_as_interrupted() {
    use std::time::{Duration, Instant};

    let (base_url, server) = crate::ai::sse_truncated_server();
    let (mut app, dir) = fixture(
        "x
",
    );
    let mut config = crate::ai::test_config(base_url);
    config.stream = true;
    app.state.config.ai = config;
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.input = "hello".to_string();
    app.state.ai.cursor = 5;
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.ai_stream.is_some() {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.ai_stream.is_none());
    let entries = &app.state.ai.entries;
    assert!(
        entries.iter().any(|line| line.contains("partial")),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|line| line == "[stream interrupted]"),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|line| line.contains("ended before [DONE]")),
        "{entries:?}"
    );
    assert!(
        !app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["role"] == "assistant"),
        "a truncated reply must not enter the conversation"
    );
    server.join().expect("server");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_panel_renders_streamed_deltas_before_done() {
    use std::time::{Duration, Instant};

    let (base_url, server) = crate::ai::sse_hello_server();
    let (mut app, dir) = fixture("x\n");
    let mut config = crate::ai::test_config(base_url);
    config.stream = true;
    app.state.config.ai = config;
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.input = "hello".to_string();
    app.state.ai.cursor = 5;
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.ai_stream.is_some() {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.ai_stream.is_none());
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("hello"))
    );
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| { message["role"] == "assistant" && message["content"] == "hello" })
    );
    server.join().expect("server");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_input_accepts_shift_enter_newlines_and_enter_submits() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai.base_url.clear();
    app.dispatch(Command::AiChat).expect("ai panel");
    for ch in "first".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    for ch in "second".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(app.state.ai.input, "first\nsecond");
    assert!(app.ai_task.is_none());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.state.ai.input.is_empty());
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| { message["role"] == "user" && message["content"] == "first\nsecond" })
    );
    if let Some(task) = app.ai_task.as_ref() {
        task.cancel();
    }
    app.ai_task = None;
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn background_agent_auto_runs_read_only_tools_and_finishes() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = std::thread::spawn(move || {
        let bodies = [
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"read-1","type":"function","function":{"name":"native__read_file","arguments":"{\"path\":\"note.txt\"}"}}]}}]}"#,
            r#"{"choices":[{"message":{"content":"background done"}}]}"#,
        ];
        for body in bodies {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        }
    });
    let (mut app, dir) = fixture("x\n");
    let mut config = crate::ai::test_config(format!("http://{address}/v1"));
    config.stream = false;
    config.background_agents = true;
    app.state.config.ai = config;
    app.dispatch(Command::AiBackground)
        .expect("background prompt");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("background prompt missing")
    };
    prompt.fields[0] = "inspect the file".to_string();
    app.submit_prompt().expect("start background");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.background_agent {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.background_agent, "{:?}", app.state.ai.entries);
    assert!(app.state.ai.conversation.iter().any(|message| {
        message["role"] == "tool"
            && message["content"]
                .as_str()
                .is_some_and(|text| text.contains("snapshot:"))
    }));
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("background done"))
    );
    server.join().expect("server");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn background_agent_pauses_for_a_write_and_resumes_on_approval() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = std::thread::spawn(move || {
        let bodies = [
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"write-1","type":"function","function":{"name":"native__write_file","arguments":"{\"path\":\"new.txt\",\"content\":\"created\\n\"}"}}]}}]}"#,
            r#"{"choices":[{"message":{"content":"write done"}}]}"#,
        ];
        for body in bodies {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        }
    });
    let (mut app, dir) = fixture("x\n");
    let mut config = crate::ai::test_config(format!("http://{address}/v1"));
    config.stream = false;
    config.background_agents = true;
    app.state.config.ai = config;
    app.dispatch(Command::AiBackground)
        .expect("background prompt");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("background prompt missing")
    };
    prompt.fields[0] = "create a file".to_string();
    app.submit_prompt().expect("start background");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.state.ai.pending.is_empty() {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.background_agent);
    assert!(app.state.ai.pending.front().is_some());
    assert!(!dir.join("new.txt").exists());
    app.dispatch(Command::AiPanel).expect("open panel");
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('y'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.background_agent {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("new.txt")).expect("written file"),
        "created\n"
    );
    server.join().expect("server");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_panel_commands_copy_clear_resize_and_scroll() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.entries.push("hello".to_string());
    app.dispatch(Command::AiCopy).expect("copy");
    assert!(app.state.clipboard.contains("hello"));
    app.dispatch(Command::AiWider).expect("wider");
    assert!(app.state.config.ai.panel_width > 20);
    app.dispatch(Command::AiScrollRight).expect("scroll");
    assert_eq!(app.state.ai.scroll_x, 4);
    app.dispatch(Command::AiClear).expect("clear");
    assert_eq!(app.state.ai.entries.len(), 1);
    assert!(app.state.ai.conversation.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_tool_policy_filters_the_tool_definitions() {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping AI tool policy: python3 unavailable");
        return;
    }
    let (mut app, dir) = fixture("x\n");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp.py");
    let mcp = crate::mcp::McpServerConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        cwd: None,
        env: BTreeMap::from([("MOCK_MCP_MODE".to_string(), "normal".to_string())]),
        url: None,
        allow_sampling: false,
        auth_token_env: None,
        headers: BTreeMap::new(),
    };
    app.state
        .config
        .mcp
        .servers
        .insert("mock".to_string(), mcp.clone());
    app.mcp = Some(crate::mcp::McpManager::new(
        BTreeMap::from([("mock".to_string(), mcp)]),
        dir.clone(),
    ));
    app.mcp
        .as_mut()
        .expect("mcp")
        .list_tools("mock")
        .expect("load tools");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.pump_mcp();
        if app
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.tools("mock"))
            .is_some_and(|tools| !tools.is_empty())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    app.state.config.ai.allowed_tools = vec!["mock/echo".to_string()];
    app.state.config.ai.denied_tools = vec!["mock/add".to_string()];
    app.dispatch(Command::AiChat).expect("ai panel");
    app.start_ai_request();
    assert!(app.state.ai.tool_map.contains_key("mock__echo"));
    assert!(!app.state.ai.tool_map.contains_key("mock__add"));
    assert!(!app.state.ai.tool_map.contains_key("mock__read_text_file"));
    if let Some(task) = app.ai_task.as_ref() {
        task.cancel();
    }
    app.ai_task = None;
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_tool_audit_redacts_arguments_and_records_denial() {
    use crate::state::AiToolCall;

    let (mut app, dir) = fixture("x\n");
    let config_path = dir.join("config.toml");
    app.state.config_path = Some(config_path.clone());
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.pending.push_back(AiToolCall {
        id: "call-audit".to_string(),
        server: "mock".to_string(),
        tool: "echo".to_string(),
        capability: "destructive".to_string(),
        arguments: serde_json::json!({"api_token": "secret-value", "text": "hello"}),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('n'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let audit =
        std::fs::read_to_string(config_path.with_file_name("audit.jsonl")).expect("audit log");
    let record: serde_json::Value =
        serde_json::from_str(audit.lines().next().expect("record")).expect("json record");
    assert_eq!(record["event"], "decision");
    assert_eq!(record["decision"], "denied");
    assert_eq!(record["capability"], "destructive");
    assert_eq!(record["arguments"]["api_token"], "[redacted]");
    assert_eq!(record["arguments"]["text"], "hello");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn overflowing_history_is_summarized_before_the_request() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        let bodies = [
            r#"{"choices":[{"message":{"role":"assistant","content":"earlier summary"}}]}"#,
            r#"{"choices":[{"message":{"role":"assistant","content":"final answer"}}]}"#,
        ];
        for body in bodies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 16384];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let (mut app, dir) = fixture("x\n");
    let config_path = dir.join("config.toml");
    app.state.config_path = Some(config_path.clone());
    let mut config = crate::ai::test_config(format!("http://{address}/v1"));
    config.stream = false;
    config.compact_history = true;
    config.max_history_turns = 1;
    app.state.config.ai = config;
    app.state.ai.conversation.push(serde_json::json!({
        "role": "user",
        "content": "old question",
    }));
    app.state.ai.conversation.push(serde_json::json!({
        "role": "assistant",
        "content": "old answer",
    }));
    app.state.ai.conversation.push(serde_json::json!({
        "role": "user",
        "content": "new question",
    }));
    app.start_ai_request();

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline
        && !app
            .state
            .ai
            .conversation
            .iter()
            .any(|message| message["content"] == "final answer")
    {
        app.pump_ai();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["content"] == "final answer"),
        "{:?}",
        app.state.ai.conversation
    );
    let summary = app
        .state
        .ai
        .conversation
        .iter()
        .find(|message| message["role"] == "system")
        .expect("summary");
    assert!(
        summary["content"]
            .as_str()
            .is_some_and(|text| text.contains("earlier summary")),
        "{summary}"
    );
    assert!(
        !app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["content"] == "old question")
    );
    assert!(
        !app.state
            .ai
            .conversation
            .iter()
            .any(|message| message["content"] == "old answer")
    );

    let audit =
        std::fs::read_to_string(config_path.with_file_name("audit.jsonl")).expect("audit log");
    let stops: Vec<String> = audit
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| record["event"] == "model_request")
        .filter_map(|record| record["stop"].as_str().map(str::to_string))
        .collect();
    assert!(stops.contains(&"compact".to_string()), "{stops:?}");
    assert!(stops.contains(&"reply".to_string()), "{stops:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn model_trace_records_latency_usage_and_lists_in_overlay() {
    use std::time::Duration;

    let (mut app, dir) = fixture("x\n");
    let config_path = dir.join("config.toml");
    app.state.config_path = Some(config_path.clone());
    let call = crate::app::audit::ModelRequest {
        round: 2,
        model: "mock-model",
        original_messages: 10,
        sent_messages: 6,
        estimated_tokens: 1234,
        latency: Duration::from_millis(456),
        stop: "reply",
        prompt_tokens: Some(100),
        completion_tokens: Some(20),
        tool_calls: 1,
    };
    crate::app::audit::model_request(&app.state, &call);

    let audit =
        std::fs::read_to_string(config_path.with_file_name("audit.jsonl")).expect("audit log");
    let record: serde_json::Value =
        serde_json::from_str(audit.lines().next().expect("record")).expect("json record");
    assert_eq!(record["event"], "model_request");
    assert_eq!(record["round"], 2);
    assert_eq!(record["model"], "mock-model");
    assert_eq!(record["messages"]["sent"], 6);
    assert_eq!(record["estimated_tokens"], 1234);
    assert_eq!(record["latency_ms"], 456);
    assert_eq!(record["stop"], "reply");
    assert_eq!(record["prompt_tokens"], 100);
    assert_eq!(record["completion_tokens"], 20);
    assert_eq!(record["tool_calls"], 1);
    assert!(record.get("content").is_none());

    app.dispatch(Command::AiTrace).expect("trace overlay");
    let view = app.state.trace_view.as_ref().expect("trace view");
    let line = &view.lines[0];
    assert!(line.contains("r2"), "{line}");
    assert!(line.contains("mock-model"), "{line}");
    assert!(line.contains("6/10 msg"), "{line}");
    assert!(line.contains("456ms"), "{line}");
    assert!(line.contains("reply"), "{line}");
    assert!(line.contains("100+20 tok"), "{line}");
    assert!(line.contains("1 tool(s)"), "{line}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_tool_output_is_capped_before_history() {
    use crate::mcp::ToolContent;
    use crate::state::AiWaiting;

    let (mut app, dir) = fixture("x\n");
    app.state.config.ai.max_tool_output_chars = 500;
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.waiting = Some(AiWaiting {
        id: "call-cap".to_string(),
        server: "mock".to_string(),
        tool: "echo".to_string(),
        capability: "destructive".to_string(),
    });
    app.finish_ai_tool(
        "mock",
        "echo",
        &[ToolContent::Text("x".repeat(5_000))],
        false,
    );
    let text = app
        .state
        .ai
        .conversation
        .last()
        .and_then(|message| message["content"].as_str())
        .expect("tool message");
    assert!(text.ends_with("[truncated]"), "{text}");
    assert!(text.chars().count() < 1_000);
    if let Some(task) = app.ai_task.as_ref() {
        task.cancel();
    }
    app.ai_task = None;
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn denying_an_ai_tool_call_does_not_invoke_mcp() {
    use crate::state::AiToolCall;

    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.pending.push_back(AiToolCall {
        id: "call-deny".to_string(),
        server: "mock".to_string(),
        tool: "echo".to_string(),
        capability: "destructive".to_string(),
        arguments: serde_json::json!({"text": "hello"}),
    });
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('n'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(app.state.ai.pending.is_empty());
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("denied by user"))
    );
    assert_eq!(app.state.ai.conversation.last().unwrap()["role"], "tool");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ai_turn_stops_at_the_round_cap() {
    let (mut app, dir) = fixture("x\n");
    app.dispatch(Command::AiChat).expect("ai panel");
    app.state.ai.rounds = 8;
    app.start_ai_request();
    assert!(app.ai_task.is_none());
    assert!(
        app.state
            .ai
            .entries
            .iter()
            .any(|line| line.contains("stopped after 8 model rounds"))
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_command_opens_a_server_menu() {
    let (mut app, dir) = fixture("x\n");
    app.state.config.mcp.servers.insert(
        "filesystem".to_string(),
        crate::mcp::McpServerConfig {
            command: "npx".to_string(),
            args: vec!["server".to_string()],
            ..Default::default()
        },
    );
    app.dispatch(Command::Mcp).expect("mcp");
    let InputMode::Prompt(prompt) = &app.state.mode else {
        panic!("MCP menu missing")
    };
    assert_eq!(prompt.kind, crate::input::PromptKind::Mcp);
    assert!(
        prompt
            .candidates
            .iter()
            .any(|line| line.contains("filesystem") && line.contains("trust required")),
        "{:?}",
        prompt.candidates
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_add_wizard_prefills_the_workspace_directory() {
    use crate::input::PromptKind;

    let (mut app, dir) = fixture("x\n");
    app.mcp = Some(crate::mcp::McpManager::new(
        std::collections::BTreeMap::new(),
        dir.clone(),
    ));
    app.dispatch(Command::McpAdd).expect("mcp_add");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("template prompt missing")
    };
    assert_eq!(prompt.kind, PromptKind::McpAdd);
    let filesystem = prompt
        .candidates
        .iter()
        .find(|candidate| candidate.starts_with("filesystem — "))
        .cloned()
        .expect("filesystem template");
    prompt.select_candidate(&filesystem);
    app.submit_prompt().expect("template");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("path prompt missing")
    };
    assert_eq!(prompt.kind, PromptKind::McpAddPath);
    assert_eq!(
        std::fs::canonicalize(&prompt.fields[0]).expect("prefilled dir"),
        std::fs::canonicalize(&dir).expect("canonical root")
    );
    app.submit_prompt().expect("add server");
    assert!(app.state.config.mcp.servers.contains_key("filesystem"));
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("trust prompt missing after add")
    };
    assert_eq!(prompt.kind, PromptKind::McpTrust);
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Esc,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_path_prefill_uses_active_file_for_reads_and_root_for_listing() {
    use crate::mcp::{ArgKind, ToolArg};

    let (app, dir) = fixture("x\n");
    let path = ToolArg {
        name: "path".to_string(),
        description: String::new(),
        kind: ArgKind::String,
        required: true,
    };
    assert_eq!(
        std::fs::canonicalize(app.prefill_mcp_arg("read_text_file", &path)).expect("read path"),
        std::fs::canonicalize(dir.join("note.txt")).expect("canonical file")
    );
    assert_eq!(
        std::fs::canonicalize(app.prefill_mcp_arg("list_directory", &path)).expect("dir path"),
        std::fs::canonicalize(&dir).expect("canonical dir")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_denying_trust_does_not_spawn_the_server() {
    use std::collections::BTreeMap;

    let (mut app, dir) = fixture("x\n");
    let config = crate::mcp::McpServerConfig {
        command: "definitely-not-a-real-mcp-server".to_string(),
        ..Default::default()
    };
    app.state
        .config
        .mcp
        .servers
        .insert("blocked".to_string(), config.clone());
    app.mcp = Some(crate::mcp::McpManager::new(
        BTreeMap::from([("blocked".to_string(), config)]),
        dir.clone(),
    ));
    app.dispatch(Command::Mcp).expect("mcp");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("MCP menu missing")
    };
    prompt.select_candidate("blocked — trust required");
    app.submit_prompt().expect("select");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("trust prompt missing")
    };
    prompt.select_candidate("Cancel");
    app.submit_prompt().expect("cancel trust");
    assert!(!app.mcp.as_ref().expect("mcp").is_running("blocked"));
    assert!(app.state.ui.message.contains("cancelled"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_trust_tools_call_and_result_flow() {
    use crate::input::PromptKind;
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping MCP flow: python3 unavailable");
        return;
    }
    let (mut app, dir) = fixture("x\n");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp.py");
    let config = crate::mcp::McpServerConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        cwd: None,
        env: BTreeMap::from([("MOCK_MCP_MODE".to_string(), "normal".to_string())]),
        url: None,
        allow_sampling: false,
        auth_token_env: None,
        headers: BTreeMap::new(),
    };
    app.state
        .config
        .mcp
        .servers
        .insert("mock".to_string(), config.clone());
    app.mcp = Some(crate::mcp::McpManager::new(
        BTreeMap::from([("mock".to_string(), config)]),
        dir.clone(),
    ));

    app.dispatch(Command::Mcp).expect("mcp");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("MCP menu missing")
    };
    prompt.select_candidate("mock — trust required");
    app.submit_prompt().expect("select server");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("trust prompt missing")
    };
    assert_eq!(prompt.kind, PromptKind::McpTrust);
    assert!(
        !app.mcp.as_ref().expect("mcp").is_running("mock"),
        "server must not start before trust"
    );
    let trust = prompt
        .candidates
        .iter()
        .find(|candidate| candidate.starts_with("Trust and start"))
        .cloned()
        .expect("trust row");
    prompt.select_candidate(&trust);
    app.submit_prompt().expect("trust");

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.pump_mcp();
        if let InputMode::Prompt(prompt) = &app.state.mode
            && prompt.kind == PromptKind::McpTools
            && !prompt.candidates.is_empty()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("tools prompt missing")
    };
    assert_eq!(prompt.kind, PromptKind::McpTools);
    let echo = prompt
        .candidates
        .iter()
        .find(|candidate| candidate.starts_with("echo — "))
        .cloned()
        .expect("echo tool");
    prompt.select_candidate(&echo);
    app.submit_prompt().expect("select tool");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("arguments prompt missing")
    };
    prompt.fields[0] = "hello".to_string();
    prompt.cursor = prompt.fields[0].chars().count();
    app.submit_prompt().expect("arguments");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("confirm prompt missing")
    };
    assert_eq!(prompt.kind, PromptKind::McpConfirm);
    prompt.select_candidate("Cancel");
    app.submit_prompt().expect("cancel call");
    assert!(app.state.ui.message.contains("cancelled"));
    for _ in 0..10 {
        app.pump_mcp();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !app.mcp
            .as_ref()
            .expect("mcp")
            .logs("mock")
            .is_some_and(|logs| logs.iter().any(|line| line.contains("mock call echo"))),
        "cancelled call must not reach the server"
    );

    app.dispatch(Command::Mcp).expect("mcp again");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("MCP menu missing")
    };
    prompt.select_candidate("mock — trusted");
    app.submit_prompt().expect("select trusted server");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("tools prompt missing")
    };
    let echo = prompt
        .candidates
        .iter()
        .find(|candidate| candidate.starts_with("echo — "))
        .cloned()
        .expect("echo tool");
    prompt.select_candidate(&echo);
    app.submit_prompt().expect("select tool again");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("arguments prompt missing")
    };
    prompt.fields[0] = "hello".to_string();
    prompt.cursor = prompt.fields[0].chars().count();
    app.submit_prompt().expect("arguments again");
    let InputMode::Prompt(prompt) = &mut app.state.mode else {
        panic!("confirm prompt missing")
    };
    prompt.select_candidate("Call tool");
    app.submit_prompt().expect("call");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && app.state.mcp_view.is_none() {
        app.pump_mcp();
        std::thread::sleep(Duration::from_millis(10));
    }
    let view = app.state.mcp_view.as_ref().expect("result overlay");
    assert!(
        view.lines.iter().any(|line| line.contains("hello")),
        "{:?}",
        view.lines
    );
    assert_eq!(text(&app), "x\n", "MCP result must not edit the buffer");
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
    crate::app::log_keymap(Some(&config), &crate::input::KeyMap::preset(false), false);
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
fn quick_open_index_tracks_watcher_paths_without_a_rescan() {
    use crate::app::QuickOpenIndex;

    let (mut app, dir) = fixture("x\n");
    let workspace = app.state.workspaces.active().id;
    app.watchers = vec![(
        workspace,
        crate::watch::Watcher::new(&dir, None).expect("watch"),
    )];
    app.quick_open_candidates();

    let root = dir.canonicalize().expect("canonical root");
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).expect("nested");
    let deep = nested.join("deep.txt");
    std::fs::write(&deep, "x\n").expect("deep");
    app.update_quick_open(std::slice::from_ref(&deep));
    let files = app.quick_open_candidates();
    assert!(files.contains(&"nested/deep.txt".to_string()), "{files:?}");

    std::fs::remove_file(&deep).expect("remove");
    app.update_quick_open(std::slice::from_ref(&deep));
    let files = app.quick_open_candidates();
    assert!(!files.contains(&"nested/deep.txt".to_string()), "{files:?}");

    // Backends that report only the changed directory still reconcile it.
    let root_file = root.join("root.txt");
    std::fs::write(&root_file, "x\n").expect("root file");
    app.update_quick_open(std::slice::from_ref(&root));
    let files = app.quick_open_candidates();
    assert!(files.contains(&"root.txt".to_string()), "{files:?}");
    std::fs::remove_file(&root_file).expect("remove root file");
    app.update_quick_open(std::slice::from_ref(&root));
    let files = app.quick_open_candidates();
    assert!(!files.contains(&"root.txt".to_string()), "{files:?}");

    // Ignored directories never enter the list.
    std::fs::write(root.join(".gitignore"), "target/\n").expect("ignore");
    std::fs::create_dir_all(root.join("target")).expect("target");
    let built = root.join("target/out.bin");
    std::fs::write(&built, "x").expect("built");
    app.state.explorer.refresh();
    app.update_quick_open(std::slice::from_ref(&built));
    let files = app.quick_open_candidates();
    assert!(!files.contains(&"target/out.bin".to_string()), "{files:?}");

    // The cache survives an explorer revision change; only events edit it.
    app.quick_open_files = Some(QuickOpenIndex {
        files: vec!["kept.txt".to_string()],
        truncated: false,
    });
    app.state.explorer.rebuild();
    assert_eq!(app.quick_open_candidates(), vec!["kept.txt".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn quick_open_scan_reports_truncation() {
    let (app, dir) = fixture("x\n");
    std::fs::write(dir.join("a.txt"), "a\n").expect("a");
    std::fs::write(dir.join("b.txt"), "b\n").expect("b");
    let (files, truncated) = app.scan_quick_open_limit(2);
    assert_eq!(files.len(), 2);
    assert!(truncated);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recursive_watcher_lists_files_in_unexpanded_directories() {
    use std::time::{Duration, Instant};

    let (mut app, dir) = fixture("x\n");
    let workspace = app.state.workspaces.active().id;
    app.watchers = vec![(
        workspace,
        crate::watch::Watcher::new(&dir, None).expect("watch"),
    )];
    app.quick_open_candidates();

    let nested = dir.canonicalize().expect("canonical").join("nested");
    std::fs::create_dir_all(&nested).expect("nested");
    std::fs::write(nested.join("deep.txt"), "x\n").expect("deep");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut listed = false;
    while Instant::now() < deadline && !listed {
        app.pump_watcher();
        listed = app
            .quick_open_candidates()
            .iter()
            .any(|file| file == "nested/deep.txt");
        if !listed {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(listed, "nested file never reached quick open");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn verify_tool_is_registered_only_when_configured_and_allowed() {
    let (mut app, dir) = fixture("x\n");
    let verify = crate::config::VerifyConfig {
        command: "cargo".to_string(),
        args: vec!["test".to_string()],
        timeout_secs: 120,
        sandbox: false,
    };
    app.state
        .config
        .ai
        .verify
        .insert("test".to_string(), verify);

    let tools = app.ai_tool_definitions();
    let definition = tools
        .iter()
        .find(|tool| tool["function"]["name"] == "native__verify")
        .expect("verify tool");
    assert_eq!(
        definition["function"]["parameters"]["properties"]["name"]["enum"][0],
        "test"
    );

    app.state
        .config
        .ai
        .denied_tools
        .push("native/verify".to_string());
    assert!(
        !app.ai_tool_definitions()
            .iter()
            .any(|tool| tool["function"]["name"] == "native__verify")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn workspace_rules_are_injected_into_the_system_prompt_and_overlay() {
    let (mut app, dir) = fixture("x\n");
    std::fs::write(dir.join("AGENTS.md"), "Use tabs.\nPrefer small diffs.\n").expect("rules");
    std::fs::write(dir.join("MEMORY.md"), "Remember the flaky test.\n").expect("memory");

    let prompt = app.ai_system_prompt();
    assert!(
        prompt.contains("<workspace_rules source=\"AGENTS.md\">"),
        "{prompt}"
    );
    assert!(prompt.contains("Prefer small diffs."), "{prompt}");
    assert!(
        prompt.contains("<workspace_memory source=\"MEMORY.md\">"),
        "{prompt}"
    );
    assert!(prompt.contains("Remember the flaky test."), "{prompt}");

    app.state.ai.conversation.push(serde_json::json!({
        "role": "user",
        "content": "hi",
    }));
    let messages = app.ai_request_messages();
    assert_eq!(messages[0]["role"], "system");
    assert!(
        messages[0]["content"]
            .as_str()
            .is_some_and(|text| text.contains("Use tabs."))
    );

    app.dispatch(Command::AiRules).expect("rules overlay");
    let view = app.state.rules_view.as_ref().expect("rules view");
    assert!(view.lines.iter().any(|line| line.contains("AGENTS.md")));
    assert!(view.lines.iter().any(|line| line == "Use tabs."));
    assert!(view.lines.iter().any(|line| line.contains("MEMORY.md")));
    assert!(
        view.lines
            .iter()
            .any(|line| line == "Remember the flaky test.")
    );

    app.state.config.ai.workspace_rules = false;
    app.state.config.ai.workspace_memory = false;
    let prompt = app.ai_system_prompt();
    assert!(!prompt.contains("Use tabs."), "{prompt}");
    assert!(!prompt.contains("Remember the flaky test."), "{prompt}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn large_files_open_read_only_and_refuse_edits_and_saves() {
    let (mut app, dir) = fixture("x\n");
    let path = dir.join("huge.log");
    let file = std::fs::File::create(&path).expect("create");
    file.set_len(17 << 20).expect("size");
    drop(file);

    app.open_document(path.clone()).expect("prompt");
    {
        let InputMode::Prompt(prompt) = &app.state.mode else {
            panic!("large-file prompt missing");
        };
        assert_eq!(prompt.kind, crate::input::PromptKind::LargeFile);
        assert!(prompt.candidates[0].starts_with("Open read-only"));
    }
    app.submit_prompt().expect("open");

    let id = app.state.active_document_id().expect("doc");
    assert!(app.state.documents[&id].read_only);
    assert_eq!(app.state.documents[&id].len_chars(), 17 << 20);
    let before = app.state.documents[&id].len_chars();
    app.dispatch(Command::InsertText("x".to_string()))
        .expect("refused insertion");
    assert_eq!(app.state.documents[&id].len_chars(), before);
    assert!(
        app.state.ui.message.contains("read-only"),
        "{}",
        app.state.ui.message
    );
    let error = app.dispatch(Command::Save).expect_err("save refused");
    assert!(error.to_string().contains("read-only"), "{error}");
    app.dispatch(Command::GoToSymbol).expect("symbols refused");
    assert!(!app.state.mode.is_prompt());
    std::fs::remove_dir_all(&dir).ok();
}
