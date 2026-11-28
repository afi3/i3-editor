//! OpenAI-compatible SSE streaming for the AI panel.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use serde_json::{Value, json};

use super::{Reply, ToolCall, parse_reply, prepare_request};

#[derive(Debug)]
pub enum StreamEvent {
    Delta(String),
    Done(Reply),
    Error(String),
}

pub struct StreamTask {
    receiver: Receiver<StreamEvent>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl StreamTask {
    pub fn try_recv(&self) -> Option<StreamEvent> {
        self.receiver.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
    }
}

impl Drop for StreamTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn spawn_stream(
    config: crate::config::AiConfig,
    messages: Vec<Value>,
    tools: Vec<Value>,
) -> StreamTask {
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(None));
    let worker_cancel = Arc::clone(&cancel);
    let worker_child = Arc::clone(&child);
    thread::spawn(move || {
        run(
            &config,
            &messages,
            &tools,
            &sender,
            &worker_cancel,
            &worker_child,
            0,
        );
    });
    StreamTask {
        receiver,
        cancel,
        child,
    }
}

type StreamParts = (
    Box<dyn Read + Send>,
    thread::JoinHandle<String>,
    tempfile::NamedTempFile,
);

fn open_stream(
    config: &crate::config::AiConfig,
    messages: &[Value],
    tools: &[Value],
    child_slot: &Mutex<Option<Child>>,
) -> Result<StreamParts, String> {
    let prepared = prepare_request(config, messages, tools, true)?;
    let mut child = spawn_curl(&prepared.curl_config)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "curl: no stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "curl: no stderr".to_string())?;
    if let Ok(mut slot) = child_slot.lock() {
        *slot = Some(child);
    }
    let stderr_thread = thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text);
        text
    });
    Ok((Box::new(stdout), stderr_thread, prepared.body_file))
}

fn run(
    config: &crate::config::AiConfig,
    messages: &[Value],
    tools: &[Value],
    sender: &mpsc::Sender<StreamEvent>,
    cancel: &AtomicBool,
    child_slot: &Mutex<Option<Child>>,
    attempt: usize,
) {
    let (stdout, stderr_thread, _body_file) = match open_stream(config, messages, tools, child_slot)
    {
        Ok(stream) => stream,
        Err(error) => {
            let _ = sender.send(StreamEvent::Error(error));
            return;
        }
    };
    let body = match consume_stream(stdout, cancel, child_slot, sender) {
        Ok(body) => body,
        Err(error) => {
            let _ = sender.send(StreamEvent::Error(error));
            return;
        }
    };
    let wait_status = if let Ok(mut slot) = child_slot.lock() {
        slot.take().and_then(|mut child| child.wait().ok())
    } else {
        None
    };
    let stderr = stderr_thread.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        let _ = sender.send(StreamEvent::Error("AI request cancelled".to_string()));
        return;
    }
    if body.status == 0 && wait_status.is_none() {
        if retry_stream(
            config,
            attempt,
            cancel,
            true,
            body.accumulator.content.is_empty(),
            || {
                run(
                    config,
                    messages,
                    tools,
                    sender,
                    cancel,
                    child_slot,
                    attempt + 1,
                )
            },
        ) {
            return;
        }
        let _ = sender.send(StreamEvent::Error(format!(
            "AI request failed: {}",
            stderr.trim()
        )));
        return;
    }
    if body.status != 0 && !(200..300).contains(&body.status) {
        // Providers often put the reason inside the stream, which never lands
        // in `raw`; prefer it over an empty body.
        let message = body
            .sse_error
            .clone()
            .unwrap_or_else(|| error_message(&body.raw));
        let retryable = body.status == 408
            || body.status == 425
            || body.status == 429
            || (500..600).contains(&body.status);
        if retry_stream(
            config,
            attempt,
            cancel,
            retryable,
            body.accumulator.content.is_empty(),
            || {
                run(
                    config,
                    messages,
                    tools,
                    sender,
                    cancel,
                    child_slot,
                    attempt + 1,
                )
            },
        ) {
            return;
        }
        let _ = sender.send(StreamEvent::Error(format!(
            "AI request failed ({}): {message}",
            body.status
        )));
        return;
    }
    if body.saw_sse && !body.done {
        // A clean close without `[DONE]` means the reply was cut short; taking
        // it as final would persist a truncated answer.
        let empty = body.accumulator.content.is_empty();
        if retry_stream(config, attempt, cancel, true, empty, || {
            run(
                config,
                messages,
                tools,
                sender,
                cancel,
                child_slot,
                attempt + 1,
            )
        }) {
            return;
        }
        let _ = sender.send(StreamEvent::Error(
            "AI stream ended before [DONE]; the reply may be incomplete".to_string(),
        ));
        return;
    }
    if body.saw_sse
        && body.parse_failures > 0
        && body.accumulator.content.is_empty()
        && body.accumulator.calls.is_empty()
    {
        let _ = sender.send(StreamEvent::Error(format!(
            "AI stream had {} malformed event(s) and no usable reply",
            body.parse_failures
        )));
        return;
    }
    let reply = if body.saw_sse {
        body.accumulator.finish()
    } else {
        parse_reply(body.raw.trim()).map_err(|error| error.message)
    };
    match reply {
        Ok(reply) => {
            let _ = sender.send(StreamEvent::Done(reply));
        }
        Err(error) => {
            let _ = sender.send(StreamEvent::Error(error));
        }
    }
}

fn retry_stream(
    config: &crate::config::AiConfig,
    attempt: usize,
    cancel: &AtomicBool,
    retryable: bool,
    empty: bool,
    retry: impl FnOnce(),
) -> bool {
    if !retryable
        || !empty
        || attempt >= config.retry_attempts
        || !retry_delay(config, attempt, cancel)
    {
        return false;
    }
    retry();
    true
}

fn error_message(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| raw.trim().to_string())
}

fn retry_delay(config: &crate::config::AiConfig, attempt: usize, cancel: &AtomicBool) -> bool {
    let factor = 1u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
    let delay = config
        .retry_base_ms
        .saturating_mul(factor)
        .min(config.retry_max_ms);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(delay);
    while std::time::Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    !cancel.load(Ordering::Relaxed)
}

fn spawn_curl(curl_config: &str) -> Result<Child, String> {
    let mut child = Command::new("curl")
        .args([
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--no-buffer",
            "--write-out",
            "\n__I3_AI_STATUS__%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl: {error}"))?;
    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = std::io::Write::write_all(&mut stdin, curl_config.as_bytes())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("curl config: {error}"));
    }
    Ok(child)
}

struct StreamBody {
    accumulator: Accumulator,
    raw: String,
    saw_sse: bool,
    status: u16,
    /// `[DONE]` arrived; without it the stream was cut short.
    done: bool,
    /// Error reported inside the stream, which the raw body never sees.
    sse_error: Option<String>,
    /// Events that were not valid JSON; a whole reply of them means the stream
    /// was not what it claimed to be.
    parse_failures: usize,
}

/// One stream line may not exceed this. Without a bound a hostile or broken
/// endpoint can send a single enormous line and exhaust the process.
const MAX_LINE: usize = 8 << 20;

/// Read one `\n`-terminated line into `line`, refusing anything past `limit`.
/// Returns false at end of input.
fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    limit: usize,
) -> Result<bool, String> {
    line.clear();
    loop {
        let buffer = reader.fill_buf().map_err(|error| error.to_string())?;
        if buffer.is_empty() {
            return Ok(!line.is_empty());
        }
        match buffer.iter().position(|byte| *byte == b'\n') {
            Some(index) => {
                if line.len() + index > limit {
                    return Err(format!("stream line exceeds {limit} bytes"));
                }
                line.extend_from_slice(&buffer[..index]);
                reader.consume(index + 1);
                return Ok(true);
            }
            None => {
                if line.len() + buffer.len() > limit {
                    return Err(format!("stream line exceeds {limit} bytes"));
                }
                let len = buffer.len();
                line.extend_from_slice(buffer);
                reader.consume(len);
            }
        }
    }
}

/// One SSE event may span several `data:` lines; they are joined with a newline
/// and dispatched when a line that is not `data:` arrives (the blank line).
fn handle_payload(
    data: &str,
    accumulator: &mut Accumulator,
    done: &mut bool,
    sse_error: &mut Option<String>,
    parse_failures: &mut usize,
    sender: &mpsc::Sender<StreamEvent>,
) -> Result<(), String> {
    if data.trim() == "[DONE]" {
        *done = true;
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        *parse_failures += 1;
        return Ok(());
    };
    if sse_error.is_none()
        && let Some(message) = value.pointer("/error/message").and_then(Value::as_str)
    {
        *sse_error = Some(message.to_string());
    }
    if let Some(delta) = accumulator.push(&value)
        && sender.send(StreamEvent::Delta(delta)).is_err()
    {
        return Err("AI panel closed".to_string());
    }
    Ok(())
}

fn consume_stream(
    stdout: impl Read,
    cancel: &AtomicBool,
    child_slot: &Mutex<Option<Child>>,
    sender: &mpsc::Sender<StreamEvent>,
) -> Result<StreamBody, String> {
    let mut accumulator = Accumulator::default();
    let mut raw = String::new();
    let mut saw_sse = false;
    let mut done = false;
    let mut sse_error: Option<String> = None;
    let mut parse_failures = 0usize;
    let mut status = 0u16;
    let mut data_buffer: Option<String> = None;
    let mut reader = BufReader::new(stdout);
    let mut bytes = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            if let Ok(mut slot) = child_slot.lock()
                && let Some(child) = slot.as_mut()
            {
                let _ = child.kill();
            }
            return Err("AI request cancelled".to_string());
        }
        let more = read_line_bounded(&mut reader, &mut bytes, MAX_LINE)?;
        if !more {
            if let Some(data) = data_buffer.take() {
                handle_payload(
                    &data,
                    &mut accumulator,
                    &mut done,
                    &mut sse_error,
                    &mut parse_failures,
                    sender,
                )?;
            }
            break;
        }
        let line = String::from_utf8_lossy(&bytes);
        if let Some(code) = line.strip_prefix("__I3_AI_STATUS__") {
            status = code.trim().parse().unwrap_or(0);
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            saw_sse = true;
            // SSE strips one optional space after the colon.
            let data = data.strip_prefix(' ').unwrap_or(data);
            match &mut data_buffer {
                Some(buffer) => {
                    buffer.push('\n');
                    buffer.push_str(data);
                }
                None => data_buffer = Some(data.to_string()),
            }
            continue;
        }
        if let Some(data) = data_buffer.take() {
            handle_payload(
                &data,
                &mut accumulator,
                &mut done,
                &mut sse_error,
                &mut parse_failures,
                sender,
            )?;
        }
        if !saw_sse && !line.trim().is_empty() {
            raw.push_str(&line);
            raw.push('\n');
        }
    }
    Ok(StreamBody {
        accumulator,
        raw,
        saw_sse,
        status,
        done,
        sse_error,
        parse_failures,
    })
}

#[derive(Default)]
struct Accumulator {
    content: String,
    calls: BTreeMap<usize, PartialCall>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

impl Accumulator {
    fn push(&mut self, value: &Value) -> Option<String> {
        if let Some(prompt) = value
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
        {
            self.prompt_tokens = Some(prompt);
        }
        if let Some(completion) = value
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
        {
            self.completion_tokens = Some(completion);
        }
        let delta = value.pointer("/choices/0/delta")?;
        let mut text = None;
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            self.content.push_str(content);
            if !content.is_empty() {
                text = Some(content.to_string());
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entry = self.calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    entry.id.push_str(id);
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        entry.name.push_str(name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        entry.arguments.push_str(arguments);
                    }
                }
            }
        }
        text
    }

    fn finish(self) -> Result<Reply, String> {
        let content = (!self.content.is_empty()).then_some(self.content);
        let mut tool_calls = Vec::new();
        for (index, call) in self.calls {
            let arguments = serde_json::from_str::<Value>(&call.arguments)
                .map_err(|error| format!("tool arguments for {}: {error}", call.name))?;
            let id = if call.id.is_empty() {
                format!("stream-call-{index}")
            } else {
                call.id
            };
            tool_calls.push(ToolCall {
                id,
                name: call.name,
                arguments,
            });
        }
        if content.is_none() && tool_calls.is_empty() {
            return Err("AI response has neither content nor tool calls".to_string());
        }
        let message = json!({
            "role": "assistant",
            "content": content,
            "tool_calls": tool_calls.iter().map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments.to_string()
                    }
                })
            }).collect::<Vec<_>>()
        });
        Ok(Reply {
            message,
            content,
            tool_calls,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn consume(raw: &str) -> StreamBody {
        let (sender, _receiver) = mpsc::channel::<StreamEvent>();
        consume_stream(
            Cursor::new(raw.as_bytes().to_vec()),
            &AtomicBool::new(false),
            &Mutex::new(None),
            &sender,
        )
        .expect("stream body")
    }

    #[test]
    fn a_stream_without_done_is_marked_truncated() {
        let body = consume("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n");
        assert!(body.saw_sse);
        assert!(!body.done, "a missing [DONE] must be visible");
        assert_eq!(body.accumulator.content, "hi");
    }

    #[test]
    fn a_completed_stream_is_marked_done() {
        let body =
            consume("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n");
        assert!(body.done);
    }

    #[test]
    fn multi_line_data_fields_form_one_event() {
        // Consecutive `data:` lines are one event; the blank line dispatches it.
        let body = consume(
            "data: {\"choices\":[{\"delta\":\ndata: {\"content\":\"joined\"}}]}\n\ndata: [DONE]\n\n",
        );
        assert_eq!(body.parse_failures, 0);
        assert_eq!(body.accumulator.content, "joined");
        assert!(body.done);
    }

    #[test]
    fn malformed_events_are_counted() {
        let body = consume("data: not-json\n\ndata: [DONE]\n\n");
        assert_eq!(body.parse_failures, 1);
    }

    #[test]
    fn an_error_payload_is_captured_from_the_stream() {
        let body = consume("data: {\"error\":{\"message\":\"quota exhausted\"}}\n\n");
        assert_eq!(body.sse_error.as_deref(), Some("quota exhausted"));
    }

    #[test]
    fn a_line_past_the_limit_is_refused() {
        let mut line = Vec::new();
        let mut reader = Cursor::new(b"1234567890\n".to_vec());
        let error = read_line_bounded(&mut reader, &mut line, 4).expect_err("too long");
        assert!(error.contains("exceeds"), "{error}");

        let mut reader = Cursor::new(b"abc\ndef\n".to_vec());
        assert!(read_line_bounded(&mut reader, &mut line, 8).expect("line"));
        assert_eq!(line, b"abc");
    }
}
