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
/// Serializes tests that redirect the logger: the override is process-global,
/// so two of them would otherwise write into each other's files.
#[cfg(test)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Points the logger at `path` until the returned guard drops. Holding the
/// guard is what keeps the parallel test threads from interleaving.
#[cfg(test)]
pub(crate) struct PathGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for PathGuard {
    fn drop(&mut self) {
        if let Ok(mut path) = TEST_PATH.lock() {
            *path = None;
        }
    }
}

#[cfg(test)]
pub(crate) fn use_path_for_test(path: Option<PathBuf>) -> PathGuard {
    let lock = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *TEST_PATH.lock().expect("log path override") = path;
    PathGuard { _lock: lock }
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
    let metadata = std::fs::metadata(path).ok();
    let rotated = metadata.as_ref().is_some_and(|meta| meta.len() > MAX_BYTES);
    if rotated {
        let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
    }
    // Tighten a file that an older build, a copy, or a different umask left
    // group-readable: entries contain tool names and paths. A rotation creates
    // the file again under the umask, so it always needs the chmod.
    let needs_private = rotated
        || metadata.as_ref().is_none_or(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o077 != 0
            }
            #[cfg(not(unix))]
            {
                let _ = meta;
                false
            }
        });
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
    #[cfg(unix)]
    if needs_private {
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
    #[cfg(unix)]
    fn a_rotated_log_keeps_its_private_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("i3-log-mode-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("log.jsonl");
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

        append(&path, "after", json!({})).expect("append");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the rotated log must not reopen world-readable"
        );
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

#[test]
#[cfg(unix)]
fn an_existing_shared_log_is_tightened() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("i3-log-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("log.jsonl");
    std::fs::write(&path, "").expect("seed");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

    append(&path, "note", json!({})).expect("append");
    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "an existing shared log must be tightened");
    std::fs::remove_dir_all(&dir).ok();
}
