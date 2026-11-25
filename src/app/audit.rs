//! Append-only AI tool-call audit log.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
