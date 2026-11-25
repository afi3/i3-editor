//! Native file and hashline editing primitives for the AI harness.
//!
//! Hashes are short SHA-256 prefixes over line content. A file snapshot is
//! checked before any edit; every edit is validated against the original line
//! hash before an atomic write.

use std::fs;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SERVER: &str = "native";
pub const MAX_FILE_BYTES: u64 = 16 << 20;
pub const READ_LINES: usize = 400;
const SEARCH_RESULTS: usize = 200;

pub fn capability(tool: &str) -> &'static str {
    match tool {
        "read_file" | "search" => "read-only",
        "edit_file" => "destructive",
        _ => "unknown",
    }
}

pub fn description(tool: &str) -> &'static str {
    match tool {
        "read_file" => "Read a file with a snapshot and per-line hashes",
        "edit_file" => "Replace lines validated by snapshot and line hashes",
        "search" => "Search project files with ignore and regex",
        _ => "Native tool",
    }
}

pub fn tools() -> Vec<(&'static str, Value)> {
    vec![
        ("read_file", read_file_schema()),
        ("edit_file", edit_file_schema()),
        ("search", search_schema()),
    ]
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

pub fn edit_file(root: &Path, arguments: &Value) -> Result<String, String> {
    let raw = string_arg(arguments, "path")?;
    let path = resolve_path(root, raw)?;
    let expected_snapshot = string_arg(arguments, "snapshot")?;
    let text = read_text(&path)?;
    let actual_snapshot = snapshot(&text);
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
    let lines = line_contents(&text);
    let mut segments: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let crlf = text.contains("\r\n");
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
        segments[line - 1] = replacement_text(replacement, ending, crlf);
    }
    let new_text = segments.concat();
    crate::editor::document::write_atomic(&path, new_text.as_bytes())
        .map_err(|error| format!("write {raw}: {error}"))?;
    Ok(format!(
        "edited {raw}: {} line(s)\nsnapshot: {}",
        replacements.len(),
        snapshot(&new_text)
    ))
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

fn snapshot(text: &str) -> String {
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
        edit_file(&root, &edit).expect("edit");
        let text = fs::read_to_string(&path).expect("text");
        assert_eq!(text, "fn main() {}\n    println!(\"hi\");\n}\n");
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
}
