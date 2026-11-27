//! Append-only structured log for events that outlive the process.
//!
//! One JSON object per line, capped at 1 MiB: the file is renamed to
//! `log.jsonl.1` once it grows past the cap, so a long session cannot fill the
//! disk. Owner-only, because entries can mention tool arguments and paths.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const MAX_BYTES: u64 = 1 << 20;

/// `<config>/i3-editor/log.jsonl`, when a config directory is resolvable.
pub fn path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Ok(override_path) = TEST_PATH.lock()
        && let Some(path) = override_path.as_ref()
    {
        return Some(path.clone());
    }
    crate::config::config_dir().map(|dir| dir.join("log.jsonl"))
}

#[cfg(test)]
static TEST_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Point the logger at `path` for one test, or back at the real location.
#[cfg(test)]
pub(crate) fn use_path_for_test(path: Option<PathBuf>) {
    *TEST_PATH.lock().expect("log path override") = path;
}

/// Record one event; failures are ignored on purpose, logging must never abort
/// an edit or a request.
pub fn event(name: &str, data: Value) {
    let Some(path) = path() else {
        return;
    };
    let _ = append(&path, name, data);
}

/// Append `name` plus `data`'s fields as one JSON line.
pub fn append(path: &Path, name: &str, data: Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let created = !path.exists();
    if std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES {
        let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
    }
    let mut record = json!({"ts_ms": now_ms(), "event": name});
    if let (Some(object), Value::Object(extra)) = (record.as_object_mut(), data) {
        for (key, value) in extra {
            object.insert(key, value);
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(format!("{record}\n").as_bytes())?;
    let _ = file.sync_data();
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_events_are_one_json_object_per_line() {
        let dir = std::env::temp_dir().join(format!("i3-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("log.jsonl");

        append(&path, "first", json!({"a": 1})).expect("first");
        append(&path, "second", json!({"b": "x"})).expect("second");

        let text = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let first: Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["event"], "first");
        assert!(first["ts_ms"].is_number());
        let second: Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(second["b"], "x");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_full_log_is_rotated_once() {
        let dir = std::env::temp_dir().join(format!("i3-log-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("log.jsonl");
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).expect("seed");

        append(&path, "after", json!({})).expect("append");
        assert!(
            dir.join("log.jsonl.1").exists(),
            "old log kept for inspection"
        );
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text.lines().count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
