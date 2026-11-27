//! Append-only AI tool-call audit log.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::state::{AiToolCall, AiWaiting, AppState};

pub(crate) fn decision(state: &AppState, call: &AiToolCall, decision: &str) {
    write(
        state,
        json!({
            "time_unix": now(),
            "event": "decision",
            "tool_call_id": call.id,
            "server": call.server,
            "tool": call.tool,
            "capability": call.capability,
            "arguments": redact(&call.arguments),
            "decision": decision,
        }),
    );
}

pub(crate) fn result(state: &AppState, call: &AiWaiting, outcome: &str, detail: Option<&str>) {
    write(
        state,
        json!({
            "time_unix": now(),
            "event": "result",
            "tool_call_id": call.id,
            "server": call.server,
            "tool": call.tool,
            "capability": call.capability,
            "outcome": outcome,
            "detail": detail.map(|text| crate::ai::truncate_text(text, 500)),
        }),
    );
}

pub(crate) fn blocked(
    state: &AppState,
    id: &str,
    server: &str,
    tool: &str,
    arguments: &Value,
    reason: &str,
) {
    write(
        state,
        json!({
            "time_unix": now(),
            "event": "decision",
            "tool_call_id": id,
            "server": server,
            "tool": tool,
            "capability": "unavailable",
            "arguments": redact(arguments),
            "decision": "blocked",
            "detail": reason,
        }),
    );
    crate::logging::event(
        "tool_blocked",
        json!({
            "tool_call_id": id,
            "server": server,
            "tool": tool,
            "reason": reason,
        }),
    );
}

pub(crate) struct ModelRequest<'a> {
    pub round: usize,
    pub model: &'a str,
    pub original_messages: usize,
    pub sent_messages: usize,
    pub estimated_tokens: usize,
    pub latency: Duration,
    pub stop: &'a str,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub tool_calls: usize,
}

pub(crate) fn model_request(state: &AppState, call: &ModelRequest<'_>) {
    write(
        state,
        json!({
            "time_unix": now(),
            "event": "model_request",
            "round": call.round,
            "model": call.model,
            "messages": {
                "original": call.original_messages,
                "sent": call.sent_messages,
            },
            "estimated_tokens": call.estimated_tokens,
            "latency_ms": u64::try_from(call.latency.as_millis()).unwrap_or(u64::MAX),
            "stop": call.stop,
            "prompt_tokens": call.prompt_tokens,
            "completion_tokens": call.completion_tokens,
            "tool_calls": call.tool_calls,
        }),
    );
    crate::logging::event(
        "model_request",
        json!({
            "round": call.round,
            "model": call.model,
            "estimated_tokens": call.estimated_tokens,
            "latency_ms": u64::try_from(call.latency.as_millis()).unwrap_or(u64::MAX),
            "prompt_tokens": call.prompt_tokens,
            "completion_tokens": call.completion_tokens,
        }),
    );
}

/// Recent model-request events for the trace overlay, oldest first. Reads only
/// the tail of the audit log so a long-running file stays cheap to inspect.
pub(crate) fn recent_model_traces(state: &AppState, limit: usize) -> Vec<String> {
    let Some(path) = audit_path(state) else {
        return Vec::new();
    };
    let Ok(mut file) = File::open(&path) else {
        return Vec::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let tail = 64 * 1024;
    let start = length.saturating_sub(tail);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let mut out = Vec::new();
    for line in lines.iter().rev() {
        if out.len() >= limit {
            break;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("event").and_then(Value::as_str) != Some("model_request") {
            continue;
        }
        out.push(format_trace(&value));
    }
    out.reverse();
    out
}

fn format_trace(value: &Value) -> String {
    let time = value.get("time_unix").and_then(Value::as_u64).unwrap_or(0);
    let round = value.get("round").and_then(Value::as_u64).unwrap_or(0);
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let sent = value
        .pointer("/messages/sent")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let original = value
        .pointer("/messages/original")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let estimated = value
        .get("estimated_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let latency = value.get("latency_ms").and_then(Value::as_u64).unwrap_or(0);
    let stop = value.get("stop").and_then(Value::as_str).unwrap_or("?");
    let prompt = value
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion = value
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tools = value.get("tool_calls").and_then(Value::as_u64).unwrap_or(0);
    format!(
        "t={time} r{round} {model} | {sent}/{original} msg | ~{estimated} tok | {latency}ms | {stop} | {prompt}+{completion} tok | {tools} tool(s)"
    )
}

fn write(state: &AppState, record: Value) {
    let Some(path) = audit_path(state) else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = writeln!(file, "{line}");
    }
}

fn audit_path(state: &AppState) -> Option<PathBuf> {
    let config = state.config_path.as_ref()?;
    Some(config.parent()?.join("audit.jsonl"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if lower.contains("token")
                        || lower.contains("secret")
                        || lower.contains("password")
                        || lower.contains("authorization")
                        || lower.contains("api_key")
                    {
                        (key.clone(), Value::String("[redacted]".to_string()))
                    } else {
                        (key.clone(), redact(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        Value::String(text) => Value::String(crate::ai::truncate_text(text, 500)),
        other => other.clone(),
    }
}
