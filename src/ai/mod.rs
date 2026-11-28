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
pub mod rules;

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
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
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

fn resolve_api_key(config: &AiConfig) -> Result<String, String> {
    let service = config.keyring_service.trim();
    let account = config.keyring_account.trim();
    if !service.is_empty() || !account.is_empty() {
        if service.is_empty() || account.is_empty() {
            return Err("AI keyring_service and keyring_account must both be set".to_string());
        }
        return keyring_lookup(service, account);
    }
    if !config.api_key.trim().is_empty() {
        return Ok(config.api_key.trim().to_string());
    }
    Ok(std::env::var(&config.api_key_env).unwrap_or_default())
}

fn keyring_lookup(service: &str, account: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("security");
        command.args(["find-generic-password", "-s", service, "-a", account, "-w"]);
        keyring_command(command)
    }
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("secret-tool");
        command.args(["lookup", "service", service, "account", account]);
        keyring_command(command)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (service, account);
        Err("OS keyring lookup is unsupported on this platform".to_string())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn keyring_command(mut command: Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("keyring lookup: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "keyring lookup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() {
        return Err("keyring returned an empty key".to_string());
    }
    Ok(key)
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
        body["stream_options"] = json!({"include_usage": true});
    }
    let body_text = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    let mut body_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    body_file
        .write_all(&body_text)
        .map_err(|error| error.to_string())?;
    let key = resolve_api_key(config)?;
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

/// `{base_url}/embeddings`, keeping the configured API root intact.
fn embeddings_url(base_url: &str) -> String {
    format!("{}/embeddings", base_url.trim_end_matches('/'))
}

/// Embed every input with `model` in one request. Vectors come back in input
/// order even when the provider reorders `data`.
///
/// Test-gated until the semantic pipeline is wired to the app; the transport
/// itself is the same curl-config pattern as `prepare_request`.
pub fn embed(config: &AiConfig, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if config.base_url.trim().is_empty() || model.trim().is_empty() {
        return Err("AI base_url and embedding model must be configured".to_string());
    }
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let body_text = serde_json::to_vec(&json!({"model": model, "input": inputs}))
        .map_err(|error| error.to_string())?;
    let mut body_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    body_file
        .write_all(&body_text)
        .map_err(|error| error.to_string())?;
    let key = resolve_api_key(config)?;
    let mut curl_config = String::new();
    curl_config.push_str(&format!(
        "url = {}\n",
        quote(&embeddings_url(&config.base_url))
    ));
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

    // Same policy as chat: retry_attempts extra tries with a fixed backoff. A
    // single 5xx used to discard an entire index build.
    let attempts = config.retry_attempts.saturating_add(1).max(1);
    let mut last_error = "embeddings failed".to_string();
    for attempt in 0..attempts {
        match embed_once(&curl_config, inputs) {
            Ok(vectors) => return Ok(vectors),
            Err(error) => last_error = error,
        }
        if attempt + 1 < attempts {
            thread::sleep(Duration::from_millis(config.retry_base_ms.max(10)));
        }
    }
    Err(last_error)
}

fn embed_once(curl_config: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let mut child = Command::new("curl")
        .args(["--silent", "--show-error", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl: {error}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(curl_config.as_bytes())
            .map_err(|error| format!("curl stdin: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("curl: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("embeddings failed: {}", stderr.trim()));
    }
    let reply: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("embeddings reply: {error}"))?;
    let data = reply
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "embeddings reply has no data".to_string())?;
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; inputs.len()];
    for (position, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .map_or(position, |index| index as usize);
        let Some(values) = item.get("embedding").and_then(Value::as_array) else {
            continue;
        };
        let vector: Vec<f32> = values
            .iter()
            .filter_map(Value::as_f64)
            .map(|value| value as f32)
            .collect();
        if index < vectors.len() && !vector.is_empty() {
            vectors[index] = Some(vector);
        }
    }
    vectors
        .into_iter()
        .map(|vector| vector.ok_or_else(|| "embeddings reply is missing an input".to_string()))
        .collect()
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
    let prompt_tokens = value
        .pointer("/usage/prompt_tokens")
        .and_then(Value::as_u64);
    let completion_tokens = value
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_u64);
    Ok(Reply {
        message,
        content,
        tool_calls,
        prompt_tokens,
        completion_tokens,
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

#[derive(Clone, Debug)]
pub(crate) struct TrimPlan {
    pub(crate) system: Vec<Value>,
    pub(crate) kept: Vec<Value>,
    pub(crate) dropped: Vec<Value>,
    pub(crate) estimated_tokens: usize,
}

pub(crate) fn trim_plan(messages: &[Value], tools: &[Value], config: &AiConfig) -> TrimPlan {
    let system_count = messages
        .iter()
        .take_while(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        .count();
    let (system, rest) = messages.split_at(system_count);
    let system_tokens = estimate_messages(system);
    let tool_tokens = tools.iter().map(estimate_tokens).sum::<usize>() + tools.len();
    let budget = config
        .max_context_tokens
        .saturating_sub(tool_tokens)
        .saturating_sub(system_tokens)
        .saturating_sub(128)
        .max(1);
    let mut groups = split_turns(rest);
    let keep = config.max_history_turns.clamp(1, 500);
    let mut dropped = Vec::new();
    if groups.len() > keep {
        dropped.extend(
            groups
                .drain(..groups.len() - keep)
                .flatten()
                .collect::<Vec<Value>>(),
        );
    }
    let mut selected_start = groups.len();
    let mut used = 0;
    for index in (0..groups.len()).rev() {
        let cost = estimate_messages(&groups[index]);
        if index + 1 == groups.len() || used + cost <= budget {
            used += cost;
            selected_start = index;
        } else {
            break;
        }
    }
    dropped.extend(
        groups[..selected_start]
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<Value>>(),
    );
    let mut kept: Vec<Value> = groups[selected_start..].iter().flatten().cloned().collect();
    if estimate_messages(&kept) > budget {
        trim_large_contents(&mut kept, budget, config.max_tool_output_chars);
    }
    TrimPlan {
        estimated_tokens: estimate_messages(system) + estimate_messages(&kept),
        system: system.to_vec(),
        kept,
        dropped,
    }
}

fn trim_messages(messages: &[Value], tools: &[Value], config: &AiConfig) -> Vec<Value> {
    let plan = trim_plan(messages, tools, config);
    let mut out = plan.system;
    out.extend(plan.kept);
    out
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrimStats {
    pub original: usize,
    pub sent: usize,
    pub estimated_tokens: usize,
}

/// Stats for one prepared request. Deterministic and side-effect free, so the
/// caller can record context drops without changing the request path.
pub(crate) fn trim_stats(messages: &[Value], tools: &[Value], config: &AiConfig) -> TrimStats {
    let plan = trim_plan(messages, tools, config);
    TrimStats {
        original: messages.len(),
        sent: plan.system.len() + plan.kept.len(),
        estimated_tokens: plan.estimated_tokens,
    }
}

/// Build the one-shot summarization request for turns that trimming would drop.
/// Message content is flattened to text so tool-call ids cannot make the
/// request invalid; the whole transcript is capped by `max_chars`.
pub(crate) fn compaction_messages(dropped: &[Value], max_chars: usize) -> Vec<Value> {
    let lines: Vec<String> = dropped
        .iter()
        .flat_map(|message| {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("message");
            let mut lines = vec![format!("[{role}]")];
            if let Some(content) = message.get("content").and_then(Value::as_str) {
                lines.push(content.to_string());
            }
            lines.extend(
                message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|call| {
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let arguments = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        format!("tool call {name}: {arguments}")
                    }),
            );
            lines
        })
        .collect();
    let transcript = truncate_text(&lines.join("\n"), max_chars);
    vec![
        json!({
            "role": "system",
            "content": "You summarize an earlier conversation for context. Preserve user goals, decisions, file paths, tool outcomes, and unresolved work. Do not add new plans or invent facts. Reply with plain text only."
        }),
        json!({
            "role": "user",
            "content": format!("Summarize this earlier conversation:\n{transcript}")
        }),
    ]
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
/// Sends one delta, then closes without `[DONE]`: a stream that was cut short.
pub(crate) fn sse_truncated_server() -> (String, std::thread::JoinHandle<()>) {
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
                b"HTTP/1.1 200 OK
Content-Type: text/event-stream
Connection: close

",
            )
            .expect("headers");
        stream
            .write_all(br#"data: {"choices":[{"delta":{"content":"partial"}}]}"#)
            .expect("chunk");
        stream.write_all(b"\n\n").expect("terminator");
        stream.flush().expect("flush");
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
    fn keeps_the_system_prompt_while_trimming_history() {
        let config = AiConfig {
            max_history_turns: 1,
            max_context_tokens: 16_000,
            ..AiConfig::default()
        };
        let messages = vec![
            json!({"role": "system", "content": "rules stay"}),
            json!({"role": "user", "content": "old"}),
            json!({"role": "assistant", "content": "old answer"}),
            json!({"role": "user", "content": "new"}),
        ];
        let trimmed = trim_messages(&messages, &[], &config);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0]["role"], "system");
        assert_eq!(trimmed[0]["content"], "rules stay");
        assert_eq!(trimmed[1]["content"], "new");
    }

    #[test]
    fn trim_stats_report_context_drops() {
        let config = AiConfig {
            max_history_turns: 1,
            ..AiConfig::default()
        };
        let messages = vec![
            json!({"role": "system", "content": "rules"}),
            json!({"role": "user", "content": "old"}),
            json!({"role": "assistant", "content": "old answer"}),
            json!({"role": "user", "content": "new"}),
        ];
        let stats = trim_stats(&messages, &[], &config);
        assert_eq!(stats.original, 4);
        assert_eq!(stats.sent, 2);
        assert!(stats.estimated_tokens > 0);
    }

    #[test]
    fn trim_plan_reports_dropped_messages() {
        let config = AiConfig {
            max_history_turns: 1,
            ..AiConfig::default()
        };
        let messages = vec![
            json!({"role": "system", "content": "rules"}),
            json!({"role": "user", "content": "old"}),
            json!({"role": "assistant", "content": "old answer"}),
            json!({"role": "user", "content": "new"}),
        ];
        let plan = trim_plan(&messages, &[], &config);
        assert_eq!(plan.system.len(), 1);
        assert_eq!(plan.kept.len(), 1);
        assert_eq!(plan.kept[0]["content"], "new");
        assert_eq!(plan.dropped.len(), 2);
        assert_eq!(plan.dropped[0]["content"], "old");
        assert!(plan.estimated_tokens > 0);
    }

    #[test]
    fn compaction_messages_flatten_tool_calls() {
        let dropped = vec![
            json!({"role": "user", "content": "fix it"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}}]}),
            json!({"role": "tool", "tool_call_id": "1", "content": "file body"}),
        ];
        let messages = compaction_messages(&dropped, 4_000);
        assert_eq!(messages[0]["role"], "system");
        let text = messages[1]["content"].as_str().expect("content");
        assert!(text.contains("[user]"), "{text}");
        assert!(text.contains("fix it"), "{text}");
        assert!(text.contains("tool call read_file"), "{text}");
        assert!(text.contains("file body"), "{text}");
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
