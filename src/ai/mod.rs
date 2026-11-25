//! Minimal OpenAI-compatible chat client.
//!
//! Network I/O runs through `curl`, matching the updater/installer policy and
//! avoiding a second TLS stack. The panel uses SSE streaming by default and
//! falls back to a single JSON response when the provider does not stream.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::AiConfig;

mod stream;
pub use stream::{StreamEvent, StreamTask, spawn_stream};

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub message: Value,
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

pub struct Task {
    rx: Receiver<Result<Reply, String>>,
    cancel: Arc<AtomicBool>,
}

impl Task {
    pub fn try_recv(&self) -> Result<Result<Reply, String>, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn poll_task(task: &Task) -> Option<Result<Reply, String>> {
    match task.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            Some(Err("AI worker stopped".to_string()))
        }
    }
}

pub fn spawn_chat(config: AiConfig, messages: Vec<Value>, tools: Vec<Value>) -> Task {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    thread::spawn(move || {
        let result = chat_with_cancel(&config, &messages, &tools, &worker_cancel);
        let _ = tx.send(result);
    });
    Task { rx, cancel }
}

/// Keep the one-shot helper for tests.
#[cfg(test)]
pub fn ask(config: &AiConfig, prompt: &str) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("AI prompt is empty".to_string());
    }
    let reply = chat_with_cancel(
        config,
        &[json!({"role": "user", "content": prompt})],
        &[],
        &AtomicBool::new(false),
    )?;
    reply
        .content
        .ok_or_else(|| "AI response has no message content".to_string())
}

pub(crate) struct PreparedRequest {
    pub curl_config: String,
    pub body_file: tempfile::NamedTempFile,
}

pub(crate) fn prepare_request(
    config: &AiConfig,
    messages: &[Value],
    tools: &[Value],
    stream: bool,
) -> Result<PreparedRequest, String> {
    if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
        return Err("AI base_url and model must be configured".to_string());
    }
    let url = completions_url(&config.base_url);
    let messages = trim_messages(messages, tools, config);
    let mut body = json!({
        "model": config.model,
        "messages": messages,
        "temperature": 0.2
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    if stream {
        body["stream"] = Value::Bool(true);
    }
    let body_text = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    let mut body_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    body_file
        .write_all(&body_text)
        .map_err(|error| error.to_string())?;
    let key = if config.api_key.trim().is_empty() {
        std::env::var(&config.api_key_env).unwrap_or_default()
    } else {
        config.api_key.trim().to_string()
    };
    let mut curl_config = String::new();
    curl_config.push_str(&format!("url = {}\n", quote(&url)));
    curl_config.push_str("request = \"POST\"\n");
    curl_config.push_str("header = \"Content-Type: application/json\"\n");
    if !key.is_empty() {
        curl_config.push_str(&format!(
            "header = {}\n",
            quote(&format!("Authorization: Bearer {key}"))
        ));
    }
    curl_config.push_str(&format!(
        "data-binary = {}\n",
        quote(&format!("@{}", body_file.path().to_string_lossy()))
    ));
    curl_config.push_str(&format!("max-time = {}\n", config.timeout_secs.max(1)));
    Ok(PreparedRequest {
        curl_config,
        body_file,
    })
}

fn chat_with_cancel(
    config: &AiConfig,
    messages: &[Value],
    tools: &[Value],
    cancel: &AtomicBool,
) -> Result<Reply, String> {
    let prepared = prepare_request(config, messages, tools, false)?;
    let curl_config = prepared.curl_config;
    let _body_file = prepared.body_file;

    let attempts = config.retry_attempts.saturating_add(1).max(1);
    let mut last_error = "AI request failed".to_string();
    for attempt in 0..attempts {
        if cancel.load(Ordering::Relaxed) {
            return Err("AI request cancelled".to_string());
        }
        match request_once(&curl_config, cancel) {
            Ok(reply) => return Ok(reply),
            Err(error) => {
                last_error = error.message;
                if !error.retryable || attempt + 1 >= attempts {
                    break;
                }
                let factor = 1u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
                let delay = config
                    .retry_base_ms
                    .saturating_mul(factor)
                    .min(config.retry_max_ms);
                if !sleep_cancellable(delay, cancel) {
                    return Err("AI request cancelled".to_string());
                }
            }
        }
    }
    Err(last_error)
}

pub(crate) struct AttemptError {
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl AttemptError {
    fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            message: message.into(),
            retryable,
        }
    }
}

fn request_once(curl_config: &str, cancel: &AtomicBool) -> Result<Reply, AttemptError> {
    let mut child = Command::new("curl")
        .args([
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--write-out",
            "\n__I3_AI_STATUS__%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AttemptError::new(format!("curl: {error}"), false))?;
    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(curl_config.as_bytes())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AttemptError::new(format!("curl config: {error}"), false));
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AttemptError::new("AI request cancelled", false));
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                return Err(AttemptError::new(format!("curl: {error}"), true));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| AttemptError::new(format!("curl: {error}"), true))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let retryable = transient_curl_error(&stderr);
        return Err(AttemptError::new(
            format!("AI request failed: {}", stderr.trim()),
            retryable,
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some((body, status)) = stdout.rsplit_once("__I3_AI_STATUS__") else {
        return Err(AttemptError::new("AI response has no HTTP status", false));
    };
    let status = status.trim().parse::<u16>().unwrap_or(0);
    if !(200..300).contains(&status) {
        let message = serde_json::from_str::<Value>(body.trim())
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| body.trim().to_string());
        let retryable =
            status == 408 || status == 425 || status == 429 || (500..600).contains(&status);
        return Err(AttemptError::new(
            format!("AI request failed ({status}): {message}"),
            retryable,
        ));
    }
    parse_reply(body.trim())
}

pub(crate) fn parse_reply(body: &str) -> Result<Reply, AttemptError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| AttemptError::new(format!("AI response: {error}"), false))?;
    let message = value
        .pointer("/choices/0/message")
        .cloned()
        .ok_or_else(|| AttemptError::new("AI response has no message", false))?;
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let function = call
                .get("function")
                .ok_or_else(|| AttemptError::new("tool call has no function", false))?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let raw_arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();
            let arguments = serde_json::from_str::<Value>(&raw_arguments).map_err(|error| {
                AttemptError::new(format!("tool arguments for {name}: {error}"), false)
            })?;
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }
    if content.is_none() && tool_calls.is_empty() {
        return Err(AttemptError::new(
            "AI response has neither content nor tool calls",
            false,
        ));
    }
    Ok(Reply {
        message,
        content,
        tool_calls,
    })
}

fn transient_curl_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    [
        "timed out",
        "could not resolve",
        "connection",
        "tls",
        "empty reply",
        "recv failure",
        "send failure",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn sleep_cancellable(ms: u64, cancel: &AtomicBool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(Duration::from_millis(20)));
    }
    !cancel.load(Ordering::Relaxed)
}

fn trim_messages(messages: &[Value], tools: &[Value], config: &AiConfig) -> Vec<Value> {
    let tool_tokens = tools.iter().map(estimate_tokens).sum::<usize>() + tools.len();
    let budget = config
        .max_context_tokens
        .saturating_sub(tool_tokens)
        .saturating_sub(128)
        .max(1);
    let mut groups = split_turns(messages);
    let keep = config.max_history_turns.clamp(1, 500);
    if groups.len() > keep {
        groups.drain(..groups.len() - keep);
    }
    let mut selected = Vec::new();
    let mut used = 0;
    for group in groups.into_iter().rev() {
        let cost = estimate_messages(&group);
        if selected.is_empty() || used + cost <= budget {
            used += cost;
            selected.push(group);
        } else {
            break;
        }
    }
    selected.reverse();
    let mut messages: Vec<Value> = selected.into_iter().flatten().collect();
    if estimate_messages(&messages) > budget {
        trim_large_contents(&mut messages, budget, config.max_tool_output_chars);
    }
    messages
}

fn split_turns(messages: &[Value]) -> Vec<Vec<Value>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("user") && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }
        current.push(message.clone());
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

pub(crate) fn context_tokens(messages: &[Value]) -> usize {
    estimate_messages(messages)
}

fn estimate_messages(messages: &[Value]) -> usize {
    messages
        .iter()
        .map(|message| estimate_tokens(message) + 4)
        .sum()
}

fn estimate_tokens(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 1,
        Value::String(text) => estimate_text_tokens(text),
        Value::Array(items) => items.iter().map(estimate_tokens).sum::<usize>() + items.len(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| estimate_text_tokens(key) + estimate_tokens(value))
            .sum(),
    }
}

fn estimate_text_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4) + 4
}

fn trim_large_contents(messages: &mut [Value], budget: usize, max_tool_chars: usize) {
    let non_user = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) != Some("user"))
        .count()
        .max(1);
    let per_message = (budget.saturating_mul(4) / non_user).clamp(200, max_tool_chars.max(200));
    for message in messages.iter_mut() {
        if message.get("role").and_then(Value::as_str) == Some("user") {
            continue;
        }
        if let Some(content) = message.get("content").and_then(Value::as_str) {
            let trimmed = truncate_text(content, per_message);
            message["content"] = Value::String(trimmed);
        }
    }
    if estimate_messages(messages) > budget {
        let user_cap = budget.saturating_mul(4).max(200);
        for message in messages.iter_mut() {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            if let Some(content) = message.get("content").and_then(Value::as_str) {
                message["content"] = Value::String(truncate_text(content, user_cap));
            }
        }
    }
}

pub(crate) fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n[truncated]");
    out
}

fn completions_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

#[cfg(test)]
pub(crate) fn test_config(base_url: String) -> AiConfig {
    AiConfig {
        base_url,
        model: "mock-model".to_string(),
        api_key_env: "I3_EDITOR_TEST_AI_KEY".to_string(),
        timeout_secs: 5,
        retry_attempts: 0,
        stream: false,
        ..AiConfig::default()
    }
}

#[cfg(test)]
pub(crate) fn sse_hello_server() -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0u8; 8192];
        let _ = stream.read(&mut request).expect("read");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .expect("headers");
        for chunk in [
            r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"lo"}}]}"#,
            "data: [DONE]",
        ] {
            stream
                .write_all(format!("{chunk}\n\n").as_bytes())
                .expect("chunk");
            stream.flush().expect("flush");
        }
    });
    (format!("http://{address}/v1"), server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn asks_an_openai_compatible_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let body = r#"{"choices":[{"message":{"content":"hello from mock"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        let config = test_config(format!("http://{address}/v1"));
        let reply = ask(&config, "hello").expect("reply");
        assert_eq!(reply, "hello from mock");
        server.join().expect("server");
    }

    #[test]
    fn reports_the_rate_limit_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let body = r#"{"error":{"code":429,"message":"quota exceeded","status":"RESOURCE_EXHAUSTED"}}"#;
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        let config = test_config(format!("http://{address}/v1"));
        let error = ask(&config, "hello").expect_err("429");
        assert!(error.contains("429"), "{error}");
        assert!(error.contains("quota exceeded"), "{error}");
        server.join().expect("server");
    }

    #[test]
    fn reports_malformed_tool_arguments() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let body = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"bad","type":"function","function":{"name":"bad","arguments":"{"}}]}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        let config = test_config(format!("http://{address}/v1"));
        let task = spawn_chat(
            config,
            vec![json!({"role": "user", "content": "hello"})],
            Vec::new(),
        );
        let error = loop {
            match task.try_recv() {
                Ok(result) => break result.expect_err("malformed arguments"),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("worker disconnected")
                }
            }
        };
        assert!(error.contains("tool arguments for bad"), "{error}");
        server.join().expect("server");
    }

    #[test]
    fn retries_a_rate_limited_request_with_backoff() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for count in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut request = [0u8; 8192];
                let _ = stream.read(&mut request).expect("read");
                let (status, body) = if count == 0 {
                    (
                        "429 Too Many Requests",
                        r#"{"error":{"message":"slow down"}}"#,
                    )
                } else {
                    (
                        "200 OK",
                        r#"{"choices":[{"message":{"content":"after retry"}}]}"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        let mut config = test_config(format!("http://{address}/v1"));
        config.retry_attempts = 1;
        config.retry_base_ms = 1;
        config.retry_max_ms = 1;
        let reply = ask(&config, "hello").expect("retry reply");
        assert_eq!(reply, "after retry");
        server.join().expect("server");
    }

    #[test]
    fn streams_sse_content_and_finishes_with_a_reply() {
        let (base_url, server) = sse_hello_server();
        let mut config = test_config(base_url);
        config.stream = true;
        let task = spawn_stream(
            config,
            vec![json!({"role": "user", "content": "hello"})],
            Vec::new(),
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let mut deltas = String::new();
        let mut done = None;
        for _ in 0..1_000 {
            if let Some(event) = task.try_recv() {
                match event {
                    StreamEvent::Delta(delta) => deltas.push_str(&delta),
                    StreamEvent::Done(reply) => {
                        done = Some(reply);
                        break;
                    }
                    StreamEvent::Error(error) => panic!("{error}"),
                }
            } else if Instant::now() >= deadline {
                break;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        assert_eq!(deltas, "hello");
        assert_eq!(done.expect("done").content.as_deref(), Some("hello"));
        server.join().expect("server");
    }

    #[test]
    fn keeps_only_the_requested_history_turns() {
        let config = AiConfig {
            max_history_turns: 1,
            max_context_tokens: 16_000,
            ..AiConfig::default()
        };
        let messages = vec![
            json!({"role": "user", "content": "old"}),
            json!({"role": "assistant", "content": "old answer"}),
            json!({"role": "user", "content": "new"}),
            json!({"role": "assistant", "content": "new answer"}),
        ];
        let trimmed = trim_messages(&messages, &[], &config);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0]["content"], "new");
        assert_eq!(trimmed[1]["content"], "new answer");
    }

    #[test]
    fn trims_tool_content_to_the_context_budget() {
        let config = AiConfig {
            max_context_tokens: 512,
            max_tool_output_chars: 20_000,
            ..AiConfig::default()
        };
        let messages = vec![
            json!({"role": "user", "content": "read it"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "1", "type": "function", "function": {"name": "read", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "1", "content": "x".repeat(50_000)}),
        ];
        let trimmed = trim_messages(&messages, &[], &config);
        let text = trimmed
            .last()
            .and_then(|message| message["content"].as_str())
            .expect("tool text");
        assert!(text.ends_with("[truncated]"), "{text}");
        assert!(text.chars().count() < 50_000);
    }

    #[test]
    fn rejects_an_empty_prompt() {
        let config = AiConfig::default();
        assert!(ask(&config, "  ").is_err());
    }
}
