//! Native file and hashline editing primitives for the AI harness.
//!
//! Hashes are short SHA-256 prefixes over line content. A file snapshot is
//! checked before any edit; every edit is validated against the original line
//! hash before an atomic write.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::VerifyConfig;

pub const SERVER: &str = "native";
pub const MAX_FILE_BYTES: u64 = 16 << 20;
pub const READ_LINES: usize = 400;
const SEARCH_RESULTS: usize = 200;
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

pub fn capability(tool: &str) -> &'static str {
    match tool {
        "read_file" | "search" => "read-only",
        "edit_file" | "write_file" | "verify" => "destructive",
        _ => "unknown",
    }
}

pub fn description(tool: &str) -> &'static str {
    match tool {
        "read_file" => "Read a file with a snapshot and per-line hashes",
        "edit_file" => "Replace lines validated by snapshot and line hashes",
        "write_file" => "Create or replace a file with explicit content",
        "search" => "Search project files with ignore and regex",
        "verify" => "Run a configured verification command by name",
        _ => "Native tool",
    }
}

pub fn tools() -> Vec<(&'static str, Value)> {
    vec![
        ("read_file", read_file_schema()),
        ("edit_file", edit_file_schema()),
        ("write_file", write_file_schema()),
        ("search", search_schema()),
    ]
}

/// Schema for the configured verification commands. The model selects a name;
/// it never supplies a command line.
pub fn verify_schema(names: &[String]) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "enum": names,
                "description": "Configured verification command"
            }
        },
        "required": ["name"]
    })
}

fn read_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path relative to the workspace root"},
            "start_line": {"type": "integer", "description": "First 1-based line, default 1"},
            "end_line": {"type": "integer", "description": "Last 1-based line, inclusive"}
        },
        "required": ["path"]
    })
}

fn edit_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path relative to the workspace root"},
            "snapshot": {"type": "string", "description": "Snapshot from read_file"},
            "edits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "line": {"type": "integer", "description": "Original 1-based line"},
                        "hash": {"type": "string", "description": "Hash from read_file"},
                        "text": {"type": "string", "description": "Replacement text; empty deletes the line"}
                    },
                    "required": ["line", "hash", "text"]
                }
            }
        },
        "required": ["path", "snapshot", "edits"]
    })
}

fn write_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Path relative to the workspace root"},
            "snapshot": {"type": "string", "description": "Required when replacing an existing file"},
            "content": {"type": "string", "description": "Complete new file content"}
        },
        "required": ["path", "content"]
    })
}

fn search_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Regex searched per line"},
            "glob": {"type": "string", "description": "Optional path glob, e.g. src/**/*.rs"},
            "show_hidden": {"type": "boolean", "description": "Include hidden and ignored files"},
            "case_sensitive": {"type": "boolean", "description": "Default true"},
            "max_results": {"type": "integer", "description": "1..=1000, default 200"}
        },
        "required": ["query"]
    })
}

fn append_matches(
    output: &mut String,
    relative: &str,
    text: &str,
    regex: &regex::Regex,
    max_results: usize,
    total: &mut usize,
) -> bool {
    let mut header = false;
    for (index, line) in line_contents(text).iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        if !header {
            output.push_str(&format!("path: {relative}\nsnapshot: {}\n", snapshot(text)));
            header = true;
        }
        output.push_str(&format!("{}:{}|{}\n", index + 1, line_hash(line), line));
        *total += 1;
        if *total >= max_results {
            return true;
        }
    }
    false
}

pub fn search(root: &Path, arguments: &Value) -> Result<String, String> {
    let query = string_arg(arguments, "query")?;
    let case_sensitive = arguments
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let regex = RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| format!("invalid regex: {error}"))?;
    let show_hidden = arguments
        .get("show_hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_results = integer_arg(arguments, "max_results")
        .unwrap_or(SEARCH_RESULTS)
        .clamp(1, 1_000);
    let glob: Option<GlobMatcher> = arguments
        .get("glob")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Glob::new(value)
                .map(|glob| glob.compile_matcher())
                .map_err(|error| format!("invalid glob: {error}"))
        })
        .transpose()?;
    let root = fs::canonicalize(root).map_err(|error| format!("workspace root: {error}"))?;
    let walker = WalkBuilder::new(&root)
        .hidden(!show_hidden)
        .git_ignore(!show_hidden)
        .git_global(!show_hidden)
        .git_exclude(!show_hidden)
        .ignore(!show_hidden)
        .require_git(false)
        .build();
    let mut output = String::new();
    let mut total = 0usize;
    let mut truncated = false;
    'walk: for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        if let Some(glob) = &glob
            && !glob.is_match(&relative)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if append_matches(
            &mut output,
            &relative,
            &text,
            &regex,
            max_results,
            &mut total,
        ) {
            truncated = true;
            break 'walk;
        }
    }
    if total == 0 {
        return Ok("no matches".to_string());
    }
    if truncated {
        output.push_str("[truncated at max_results]\n");
    }
    Ok(output)
}

pub fn check_eligible(root: &Path, arguments: &Value) -> bool {
    let Some(raw) = arguments.get("path").and_then(Value::as_str) else {
        return false;
    };
    let Ok(path) = resolve_path(root, raw) else {
        return false;
    };
    path.extension().is_some_and(|extension| extension == "rs") && root.join("Cargo.toml").is_file()
}

pub struct CheckTask {
    receiver: Receiver<CheckResult>,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
pub struct CheckResult {
    pub success: bool,
    pub output: String,
}

impl CheckTask {
    pub fn spawn(root: PathBuf, timeout: Duration) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let _ = std::thread::Builder::new()
            .name("i3-cargo-check".to_string())
            .spawn(move || {
                let result = run_cargo_check(&root, timeout, &flag);
                let _ = sender.send(result);
            });
        Self { receiver, cancel }
    }

    pub fn try_recv(&self) -> Option<CheckResult> {
        self.receiver.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for CheckTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct VerifyTask {
    receiver: Receiver<VerifyResult>,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
pub struct VerifyResult {
    pub success: bool,
    pub output: String,
}

impl VerifyTask {
    pub fn spawn(root: PathBuf, name: String, spec: VerifyConfig, max_chars: usize) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let _ = std::thread::Builder::new()
            .name("i3-ai-verify".to_string())
            .spawn(move || {
                let result = run_verify(&root, &name, &spec, max_chars, &flag);
                let _ = sender.send(result);
            });
        Self { receiver, cancel }
    }

    pub fn try_recv(&self) -> Option<VerifyResult> {
        self.receiver.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for VerifyTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_verify(
    root: &Path,
    name: &str,
    spec: &VerifyConfig,
    max_chars: usize,
    cancel: &AtomicBool,
) -> VerifyResult {
    let started = Instant::now();
    let mut command = match verify_command(root, spec) {
        Ok(command) => command,
        Err(error) => {
            return VerifyResult {
                success: false,
                output: format!("verify {name}: {error}"),
            };
        }
    };
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return VerifyResult {
                success: false,
                output: format!("verify {name}: cannot start {}: {error}", spec.command),
            };
        }
    };
    let timeout = Duration::from_secs(spec.timeout_secs.max(1));
    let deadline = Instant::now() + timeout;
    // A command that logs more than a pipe buffer blocks on write and never
    // exits; the readers keep both pipes empty while the deadline is watched.
    let limit = max_chars.max(64 * 1024) * 2;
    let stdout = child.stdout.take().map(|pipe| collect_pipe(pipe, limit));
    let stderr = child.stderr.take().map(|pipe| collect_pipe(pipe, limit));
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return VerifyResult {
                success: false,
                output: format!("verify {name}: cancelled"),
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return VerifyResult {
                success: false,
                output: format!("verify {name}: timed out after {}s", spec.timeout_secs),
            };
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return VerifyResult {
                    success: false,
                    output: format!("verify {name}: {error}"),
                };
            }
        }
    };
    let mut text = String::from_utf8_lossy(&collect(stderr)).into_owned();
    text.push_str(&String::from_utf8_lossy(&collect(stdout)));
    if text.trim().is_empty() {
        text = "(no output)".to_string();
    }
    let text = crate::ai::truncate_text(&text, max_chars);
    VerifyResult {
        success: status.success(),
        output: format!(
            "verify {name}: {} in {:.1}s\n{text}",
            if status.success() { "passed" } else { "failed" },
            started.elapsed().as_secs_f64()
        ),
    }
}

/// Read a child pipe to EOF on its own thread, keeping at most `limit` bytes.
fn collect_pipe(
    mut pipe: impl std::io::Read + Send + 'static,
    limit: usize,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if kept.len() < limit {
                        let take = read.min(limit - kept.len());
                        kept.extend_from_slice(&buffer[..take]);
                    }
                }
            }
        }
        kept
    })
}

fn collect(handle: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

fn verify_command(root: &Path, spec: &VerifyConfig) -> Result<Command, String> {
    if !spec.sandbox {
        let mut command = Command::new(&spec.command);
        command.args(&spec.args);
        return Ok(command);
    }
    sandboxed_verify_command(root, spec)
}

#[cfg(target_os = "macos")]
fn sandboxed_verify_command(root: &Path, spec: &VerifyConfig) -> Result<Command, String> {
    let root = root.display().to_string().replace('"', "");
    let profile = format!(
        "(version 1)\n\
         (deny default)\n\
         (allow process*)\n\
         (allow sysctl-read)\n\
         (allow file-read*)\n\
         (allow file-write* (subpath \"{root}\") (subpath \"/tmp\") \
         (subpath \"/private/tmp\") (subpath \"/var/folders\") \
         (subpath \"/private/var/folders\") (literal \"/dev/null\") \
         (literal \"/dev/tty\"))\n"
    );
    let mut command = Command::new("sandbox-exec");
    command.args(["-p", &profile]);
    command.arg(&spec.command).args(&spec.args);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn sandboxed_verify_command(root: &Path, spec: &VerifyConfig) -> Result<Command, String> {
    let root = root.to_string_lossy().into_owned();
    let mut command = Command::new("bwrap");
    command.args([
        "--ro-bind",
        "/",
        "/",
        "--bind",
        &root,
        &root,
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--unshare-net",
        "--die-with-parent",
        "--chdir",
        &root,
    ]);
    command.arg(&spec.command).args(&spec.args);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn sandboxed_verify_command(_root: &Path, _spec: &VerifyConfig) -> Result<Command, String> {
    Err("sandboxed verification is unsupported on this platform".to_string())
}

fn run_cargo_check(root: &Path, timeout: Duration, cancel: &AtomicBool) -> CheckResult {
    let mut child = match Command::new("cargo")
        .args(["check", "--message-format=short"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return CheckResult {
                success: false,
                output: format!("cargo check: {error}"),
            };
        }
    };
    let deadline = Instant::now() + timeout;
    // Same hazard as `run_verify`: nobody reads the pipes while polling, so a
    // chatty build would block on write and be killed at the deadline.
    let stdout = child.stdout.take().map(|pipe| collect_pipe(pipe, 1 << 20));
    let stderr = child.stderr.take().map(|pipe| collect_pipe(pipe, 1 << 20));
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return CheckResult {
                success: false,
                output: "cargo check cancelled".to_string(),
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return CheckResult {
                success: false,
                output: "cargo check timed out".to_string(),
            };
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return CheckResult {
                    success: false,
                    output: format!("cargo check: {error}"),
                };
            }
        }
    };
    let mut text = String::from_utf8_lossy(&collect(stderr)).into_owned();
    text.push_str(&String::from_utf8_lossy(&collect(stdout)));
    if text.trim().is_empty() {
        text = "cargo check: ok".to_string();
    }
    CheckResult {
        success: status.success(),
        output: text,
    }
}

pub fn resolve_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("path is empty".to_string());
    }
    let root = fs::canonicalize(root).map_err(|error| format!("workspace root: {error}"))?;
    let candidate = Path::new(raw);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let path = fs::canonicalize(candidate).map_err(|error| format!("resolve {raw}: {error}"))?;
    if !path.starts_with(&root) {
        return Err(format!("path outside workspace: {raw}"));
    }
    let metadata = fs::metadata(&path).map_err(|error| format!("stat {raw}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("not a regular file: {raw}"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("file is larger than {} bytes", MAX_FILE_BYTES));
    }
    Ok(path)
}

pub(crate) fn read_source(root: &Path, raw: &str) -> Result<String, String> {
    let path = resolve_path(root, raw)?;
    read_text(&path)
}

pub fn read_file(root: &Path, arguments: &Value) -> Result<String, String> {
    let raw = string_arg(arguments, "path")?;
    let path = resolve_path(root, raw)?;
    let text = read_text(&path)?;
    let lines = line_contents(&text);
    let total = lines.len();
    let start = integer_arg(arguments, "start_line").unwrap_or(1).max(1);
    let end = integer_arg(arguments, "end_line")
        .unwrap_or_else(|| start.saturating_add(READ_LINES - 1))
        .min(total.max(1))
        .max(start);
    if total == 0 {
        return Ok(format!(
            "path: {raw}\ntotal_lines: 0\nsnapshot: {}\n(empty file)\n",
            snapshot(&text)
        ));
    }
    if start > total {
        return Err(format!("start_line {start} exceeds {total} lines"));
    }
    let end = end.min(total);
    let mut output = format!(
        "path: {raw}\ntotal_lines: {total}\nsnapshot: {}\nlines: {start}-{end}\n",
        snapshot(&text)
    );
    for (index, line) in lines
        .iter()
        .enumerate()
        .skip(start - 1)
        .take(end - start + 1)
    {
        output.push_str(&format!("{}:{}|{}\n", index + 1, line_hash(line), line));
    }
    Ok(output)
}

pub struct EditPlan {
    pub path: PathBuf,
    pub raw: String,
    pub before: String,
    pub after: String,
    pub changes: Vec<EditChange>,
}

pub struct EditChange {
    pub line: usize,
    pub before: String,
    pub after: String,
}

pub struct UndoState {
    pub raw: String,
    pub before: String,
    pub after: String,
    pub created: bool,
}

pub struct WritePlan {
    pub path: PathBuf,
    pub raw: String,
    pub before: Option<String>,
    pub after: String,
}

#[cfg(test)]
pub fn edit_file(root: &Path, arguments: &Value) -> Result<String, String> {
    edit_with_undo(root, arguments).map(|(text, _)| text)
}

pub fn edit_with_undo(root: &Path, arguments: &Value) -> Result<(String, UndoState), String> {
    let plan = plan_edit(root, arguments)?;
    crate::editor::document::write_atomic(&plan.path, plan.after.as_bytes())
        .map_err(|error| format!("write {}: {error}", plan.raw))?;
    let text = format!(
        "edited {}: {} line(s)\nsnapshot: {}",
        plan.raw,
        plan.changes.len(),
        snapshot(&plan.after)
    );
    Ok((
        text,
        UndoState {
            raw: plan.raw,
            before: plan.before,
            after: plan.after,
            created: false,
        },
    ))
}

pub fn undo_edit(root: &Path, undo: &UndoState) -> Result<PathBuf, String> {
    let path = resolve_path(root, &undo.raw)?;
    let current = read_text(&path)?;
    if snapshot(&current) != snapshot(&undo.after) {
        return Err("file changed after edit".to_string());
    }
    if undo.created {
        fs::remove_file(&path).map_err(|error| format!("undo {}: {error}", undo.raw))?;
    } else {
        crate::editor::document::write_atomic(&path, undo.before.as_bytes())
            .map_err(|error| format!("undo {}: {error}", undo.raw))?;
    }
    Ok(path)
}

pub fn plan_edit(root: &Path, arguments: &Value) -> Result<EditPlan, String> {
    let raw = string_arg(arguments, "path")?;
    let path = resolve_path(root, raw)?;
    let expected_snapshot = string_arg(arguments, "snapshot")?;
    let before = read_text(&path)?;
    let actual_snapshot = snapshot(&before);
    if expected_snapshot != actual_snapshot {
        return Err(format!(
            "stale snapshot: expected {expected_snapshot}, current {actual_snapshot}; re-read the file"
        ));
    }
    let edits = arguments
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| "edits must be an array".to_string())?;
    if edits.is_empty() {
        return Err("edits is empty".to_string());
    }
    let mut replacements = Vec::with_capacity(edits.len());
    for edit in edits {
        let line = edit
            .get("line")
            .and_then(Value::as_u64)
            .ok_or_else(|| "edit.line must be a positive integer".to_string())?
            as usize;
        let hash = edit
            .get("hash")
            .and_then(Value::as_str)
            .ok_or_else(|| "edit.hash must be a string".to_string())?;
        let text = edit
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| "edit.text must be a string".to_string())?;
        replacements.push((line, hash.to_string(), text.to_string()));
    }
    replacements.sort_by_key(|a| std::cmp::Reverse(a.0));
    if replacements.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("duplicate edit line".to_string());
    }
    let lines = line_contents(&before);
    let mut segments: Vec<String> = before.split_inclusive('\n').map(str::to_string).collect();
    let crlf = before.contains("\r\n");
    let mut changes = Vec::with_capacity(replacements.len());
    for (line, expected_hash, replacement) in &replacements {
        if *line == 0 || *line > lines.len() {
            return Err(format!("line {line} is outside 1..={}", lines.len()));
        }
        let content = lines[line - 1];
        if line_hash(content) != *expected_hash {
            return Err(format!(
                "stale hash at line {line}: expected {expected_hash}; re-read the file"
            ));
        }
        let segment = &segments[line - 1];
        let ending = if segment.ends_with("\r\n") {
            "\r\n"
        } else if segment.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let replacement = replacement_text(replacement, ending, crlf);
        changes.push(EditChange {
            line: *line,
            before: content.to_string(),
            after: replacement.clone(),
        });
        segments[line - 1] = replacement;
    }
    changes.sort_by_key(|change| change.line);
    let after = segments.concat();
    Ok(EditPlan {
        path,
        raw: raw.to_string(),
        before,
        after,
        changes,
    })
}

pub fn preview_edit(root: &Path, arguments: &Value) -> Result<String, String> {
    let plan = plan_edit(root, arguments)?;
    let mut output = format!("diff -- {}\n", plan.raw);
    for change in plan.changes {
        append_preview_change(&mut output, change);
    }
    Ok(output)
}

pub fn plan_write(root: &Path, arguments: &Value) -> Result<WritePlan, String> {
    let raw = string_arg(arguments, "path")?;
    let content = string_arg(arguments, "content")?;
    if content.len() > MAX_FILE_BYTES as usize {
        return Err(format!("content is larger than {} bytes", MAX_FILE_BYTES));
    }
    let candidate = Path::new(raw);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let (path, before) = if candidate.exists() {
        let path = resolve_path(root, raw)?;
        let before = read_text(&path)?;
        let expected = string_arg(arguments, "snapshot")?;
        let actual = snapshot(&before);
        if expected != actual {
            return Err(format!(
                "stale snapshot: expected {expected}, current {actual}; re-read the file"
            ));
        }
        (path, Some(before))
    } else {
        (resolve_new_path(root, raw)?, None)
    };
    Ok(WritePlan {
        path,
        raw: raw.to_string(),
        before,
        after: content.to_string(),
    })
}

pub fn preview_write(root: &Path, arguments: &Value) -> Result<String, String> {
    let plan = plan_write(root, arguments)?;
    let mut output = format!("diff -- {}\n", plan.raw);
    if let Some(before) = &plan.before {
        for line in before.lines() {
            output.push_str(&format!("- {line}\n"));
        }
    } else {
        output.push_str("(new file)\n");
    }
    for line in plan.after.lines() {
        output.push_str(&format!("+ {line}\n"));
    }
    Ok(output)
}

pub fn write_with_undo(root: &Path, arguments: &Value) -> Result<(String, UndoState), String> {
    let plan = plan_write(root, arguments)?;
    crate::editor::document::write_atomic(&plan.path, plan.after.as_bytes())
        .map_err(|error| format!("write {}: {error}", plan.raw))?;
    let created = plan.before.is_none();
    let text = format!(
        "{} {}\nsnapshot: {}",
        if created { "created" } else { "wrote" },
        plan.raw,
        snapshot(&plan.after)
    );
    Ok((
        text,
        UndoState {
            raw: plan.raw,
            before: plan.before.unwrap_or_default(),
            after: plan.after,
            created,
        },
    ))
}

pub(crate) fn resolve_new_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("path is empty".to_string());
    }
    let root = fs::canonicalize(root).map_err(|error| format!("workspace root: {error}"))?;
    let candidate = Path::new(raw);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| format!("path has no parent: {raw}"))?;
    let parent = fs::canonicalize(parent).map_err(|error| format!("resolve {raw}: {error}"))?;
    if !parent.starts_with(&root) {
        return Err(format!("path outside workspace: {raw}"));
    }
    let name = candidate
        .file_name()
        .ok_or_else(|| format!("path has no file name: {raw}"))?;
    Ok(parent.join(name))
}

fn append_preview_change(output: &mut String, change: EditChange) {
    output.push_str(&format!("- {} | {}\n", change.line, change.before));
    for (offset, line) in change.after.lines().enumerate() {
        let marker = if offset == 0 { '+' } else { ' ' };
        output.push_str(&format!("{marker} {} | {line}\n", change.line + offset));
    }
}

fn replacement_text(text: &str, ending: &str, crlf: bool) -> String {
    if text.is_empty() {
        return String::new();
    }
    let newline = if crlf { "\r\n" } else { "\n" };
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let body = normalized.strip_suffix('\n').unwrap_or(&normalized);
    let body = body.replace('\n', newline);
    if ending.is_empty() {
        body
    } else {
        format!("{body}{ending}")
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))
}

fn line_contents(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_inclusive('\n')
        .map(|line| line.trim_end_matches('\n').trim_end_matches('\r'))
        .collect()
}

fn line_hash(line: &str) -> String {
    short_hash(line.as_bytes(), 3)
}

pub(crate) fn snapshot(text: &str) -> String {
    short_hash(text.as_bytes(), 4)
}

fn short_hash(bytes: &[u8], bytes_len: usize) -> String {
    let digest = Sha256::digest(bytes);
    digest[..bytes_len]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn string_arg<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} must be a non-empty string"))
}

fn integer_arg(arguments: &Value, key: &str) -> Option<usize> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "i3-native-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).expect("root");
        let path = root.join("main.rs");
        fs::write(&path, "fn main() {\n    println!(\"hi\");\n}\n").expect("seed");
        (root, path)
    }

    #[test]
    fn a_noisy_verify_command_is_not_mistaken_for_a_timeout() {
        let (root, _path) = workspace();
        let spec = VerifyConfig {
            command: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "i=0; while [ $i -lt 20000 ]; do echo line-$i; i=$((i+1)); done".to_string(),
            ],
            timeout_secs: 20,
            sandbox: false,
        };
        let result = run_verify(&root, "noisy", &spec, 500, &AtomicBool::new(false));
        assert!(result.success, "{}", result.output);
        assert!(result.output.contains("passed"), "{}", result.output);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_file_hashes_lines_and_reports_a_snapshot() {
        let (root, path) = workspace();
        let output =
            read_file(&root, &serde_json::json!({"path": path.to_string_lossy()})).expect("read");
        assert!(output.contains("total_lines: 3"), "{output}");
        assert!(output.contains("1:"), "{output}");
        assert!(output.contains("|fn main() {"), "{output}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn edit_file_replaces_by_hash_and_preserves_content() {
        let (root, path) = workspace();
        let output =
            read_file(&root, &serde_json::json!({"path": path.to_string_lossy()})).expect("read");
        let snapshot = output
            .lines()
            .find_map(|line| line.strip_prefix("snapshot: "))
            .expect("snapshot");
        let line = output
            .lines()
            .find(|line| line.contains("|fn main()"))
            .expect("line");
        let (head, _) = line.split_once('|').expect("separator");
        let (number, hash) = head.split_once(':').expect("hash");
        let edit = serde_json::json!({
            "path": path.to_string_lossy(),
            "snapshot": snapshot,
            "edits": [{"line": number.parse::<usize>().unwrap(), "hash": hash, "text": "fn main() {}"}]
        });
        let preview = preview_edit(&root, &edit).expect("preview");
        assert!(preview.contains("diff --"), "{preview}");
        assert!(preview.contains("- 1 | fn main() {"), "{preview}");
        assert!(preview.contains("+ 1 | fn main() {}"), "{preview}");
        edit_file(&root, &edit).expect("edit");
        let text = fs::read_to_string(&path).expect("text");
        assert_eq!(text, "fn main() {}\n    println!(\"hi\");\n}\n");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn check_eligible_requires_a_rust_file_and_manifest() {
        let root = std::env::temp_dir().join(format!(
            "i3-native-check-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("rust");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("manifest");
        assert!(check_eligible(
            &root,
            &serde_json::json!({"path": "src/main.rs"})
        ));
        assert!(!check_eligible(
            &root,
            &serde_json::json!({"path": "src/lib.txt"})
        ));
        fs::remove_file(root.join("Cargo.toml")).expect("remove manifest");
        assert!(!check_eligible(
            &root,
            &serde_json::json!({"path": "src/main.rs"})
        ));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn write_file_creates_a_file_and_undo_removes_it() {
        let root = std::env::temp_dir().join(format!(
            "i3-native-write-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).expect("root");
        let arguments = serde_json::json!({
            "path": "new.txt",
            "content": "hello\nworld\n"
        });
        let preview = preview_write(&root, &arguments).expect("preview");
        assert!(preview.contains("(new file)"), "{preview}");
        let (_, undo) = write_with_undo(&root, &arguments).expect("write");
        assert_eq!(
            fs::read_to_string(root.join("new.txt")).expect("written"),
            "hello\nworld\n"
        );
        undo_edit(&root, &undo).expect("undo");
        assert!(!root.join("new.txt").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn search_honors_gitignore_and_returns_hashes() {
        let root = std::env::temp_dir().join(format!(
            "i3-native-search-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join(".gitignore"), "ignored.txt\n").expect("ignore");
        fs::write(root.join("src/main.rs"), "needle\n").expect("main");
        fs::write(root.join("ignored.txt"), "needle\n").expect("ignored");
        let output = search(
            &root,
            &serde_json::json!({"query": "needle", "glob": "src/**/*.rs"}),
        )
        .expect("search");
        assert!(output.contains("path: src/main.rs"), "{output}");
        assert!(!output.contains("ignored.txt"), "{output}");
        assert!(output.contains("1:"), "{output}");
        assert!(output.contains("|needle"), "{output}");
        let ignored = search(&root, &serde_json::json!({"query": "needle"})).expect("search all");
        assert!(ignored.contains("src/main.rs"), "{ignored}");
        assert!(!ignored.contains("ignored.txt"), "{ignored}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stale_line_hash_is_rejected_before_write() {
        let (root, path) = workspace();
        let original = fs::read_to_string(&path).expect("read");
        let output =
            read_file(&root, &serde_json::json!({"path": path.to_string_lossy()})).expect("read");
        let snapshot = output
            .lines()
            .find_map(|line| line.strip_prefix("snapshot: "))
            .expect("snapshot");
        let edit = serde_json::json!({
            "path": path.to_string_lossy(),
            "snapshot": snapshot,
            "edits": [{"line": 1, "hash": "000000", "text": "changed"}]
        });
        let error = edit_file(&root, &edit).expect_err("stale");
        assert!(error.contains("stale hash"), "{error}");
        assert_eq!(fs::read_to_string(&path).expect("read"), original);
        std::fs::remove_dir_all(root).ok();
    }

    fn wait_verify(task: &VerifyTask) -> VerifyResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = task.try_recv() {
                return result;
            }
            assert!(Instant::now() < deadline, "verify task did not finish");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn verify_config(command: &str, args: &[&str], timeout_secs: u64) -> VerifyConfig {
        VerifyConfig {
            command: command.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            timeout_secs,
            sandbox: false,
        }
    }

    #[test]
    fn verify_runs_a_configured_command_and_reports_failure() {
        let (root, _) = workspace();
        let task = VerifyTask::spawn(
            root.clone(),
            "fail".to_string(),
            verify_config("sh", &["-c", "echo verified; exit 3"], 5),
            1_000,
        );
        let result = wait_verify(&task);
        assert!(!result.success);
        assert!(
            result.output.contains("verify fail: failed"),
            "{}",
            result.output
        );
        assert!(result.output.contains("verified"), "{}", result.output);
        assert_eq!(capability("verify"), "destructive");
        assert_eq!(
            verify_schema(&["test".to_string()])["properties"]["name"]["enum"][0],
            "test"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn verify_times_out_and_truncates_output() {
        let (root, _) = workspace();
        let timeout = VerifyTask::spawn(
            root.clone(),
            "slow".to_string(),
            verify_config("sh", &["-c", "sleep 5"], 1),
            1_000,
        );
        let started = Instant::now();
        let result = wait_verify(&timeout);
        assert!(!result.success);
        assert!(result.output.contains("timed out"), "{}", result.output);
        assert!(started.elapsed() < Duration::from_secs(4));

        let truncated = VerifyTask::spawn(
            root.clone(),
            "noisy".to_string(),
            verify_config("sh", &["-c", "yes 0123456789 | head -n 200"], 5),
            50,
        );
        let result = wait_verify(&truncated);
        assert!(result.output.ends_with("[truncated]"), "{}", result.output);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandboxed_verify_denies_writes_outside_the_workspace() {
        if std::process::Command::new("sandbox-exec")
            .arg("-h")
            .output()
            .is_err()
        {
            eprintln!("skipping sandbox test: sandbox-exec unavailable");
            return;
        }
        let (root, _) = workspace();
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let outside = home.join(format!("i3-sandbox-{}.txt", std::process::id()));
        std::fs::remove_file(&outside).ok();
        let script = format!(
            "echo inside > inside.txt; echo outside > \"$HOME/{}\"",
            outside.file_name().expect("name").to_string_lossy()
        );
        let mut spec = verify_config("sh", &["-c", &script], 5);
        spec.sandbox = true;
        let task = VerifyTask::spawn(root.clone(), "sandbox".to_string(), spec, 2_000);
        let result = wait_verify(&task);
        assert!(!result.success, "{}", result.output);
        assert!(root.join("inside.txt").is_file());
        assert!(!outside.exists(), "sandbox allowed a write to HOME");
        std::fs::remove_dir_all(root).ok();
    }
}
