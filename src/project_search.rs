//! Background project-wide search.
//!
//! One worker thread reads the file list handed to it and streams matches back;
//! the app drains the channel once per frame. A newer search cancels the old one
//! through an atomic flag, and dropping the receiver stops it at the next send.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use crate::search_options::{Matcher, Options};

/// Matches after which the query stops and reports truncation.
pub const MAX_RESULTS: usize = 1000;
/// Files larger than this are skipped without being read.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Preview context before the match, in chars.
const PREVIEW_BEFORE: usize = 60;
/// Preview context after the match start, in chars.
const PREVIEW_AFTER: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub path: PathBuf,
    pub relative: String,
    /// Zero-based line and char column.
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

impl Match {
    /// One result row: `relative:line:column  preview`.
    pub fn label(&self) -> String {
        format!(
            "{}:{}:{}  {}",
            self.relative,
            self.line + 1,
            self.column + 1,
            self.preview
        )
    }
}

/// One file's share of the current results, in scan order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub relative: String,
    pub count: usize,
}

/// Collapse consecutive matches that belong to the same file. The worker sends
/// one batch per file, so scan order stays intact.
pub fn group_by_file(matches: &[Match]) -> Vec<FileMatches> {
    let mut groups: Vec<FileMatches> = Vec::new();
    for found in matches {
        match groups.last_mut() {
            Some(group) if group.path == found.path && group.relative == found.relative => {
                group.count += 1;
            }
            _ => groups.push(FileMatches {
                path: found.path.clone(),
                relative: found.relative.clone(),
                count: 1,
            }),
        }
    }
    groups
}

#[derive(Debug)]
pub enum Message {
    Matches(Vec<Match>),
    Done { truncated: bool },
}

pub struct Task {
    cancel: Arc<AtomicBool>,
    events: Receiver<Message>,
}

impl Task {
    /// Start searching `files` (paths relative to `root`). Returns an error for
    /// an invalid regex instead of spawning a doomed worker.
    pub fn spawn(
        root: &Path,
        files: Vec<String>,
        query: String,
        options: Options,
    ) -> Result<Self, String> {
        let matcher = Matcher::new(&query, options)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let (sender, events) = mpsc::channel();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut total = 0usize;
            let mut truncated = false;
            for relative in files {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let path = root.join(&relative);
                let Ok(metadata) = std::fs::metadata(&path) else {
                    continue;
                };
                if metadata.len() > MAX_FILE_BYTES {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let Ok(text) = String::from_utf8(bytes) else {
                    continue;
                };
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let found = scan_file(
                    &text,
                    &matcher,
                    &path,
                    &relative,
                    MAX_RESULTS.saturating_sub(total),
                );
                total += found.len();
                truncated = total >= MAX_RESULTS;
                if !found.is_empty() && sender.send(Message::Matches(found)).is_err() {
                    return;
                }
                if truncated {
                    break;
                }
            }
            let _ = sender.send(Message::Done { truncated });
        });
        Ok(Self { cancel, events })
    }

    /// Next queued message without blocking.
    pub fn try_recv(&self) -> Option<Message> {
        self.events.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn scan_file(
    text: &str,
    matcher: &Matcher,
    path: &Path,
    relative: &str,
    budget: usize,
) -> Vec<Match> {
    let mut line_starts = vec![0usize];
    for (index, ch) in text.chars().enumerate() {
        if ch == '\n' {
            line_starts.push(index + 1);
        }
    }
    let lines: Vec<&str> = text.lines().collect();
    let (ranges, _) = matcher.find(text);
    let mut cache: Option<(usize, Vec<char>)> = None;
    let mut found = Vec::new();
    for range in ranges {
        let line = line_starts
            .partition_point(|start| *start <= range.start)
            .saturating_sub(1);
        let line_text = lines.get(line).copied().unwrap_or("");
        if cache.as_ref().is_none_or(|(cached, _)| *cached != line) {
            cache = Some((line, line_text.chars().collect()));
        }
        let chars = &cache.as_ref().expect("line cache").1;
        let start = range.start.saturating_sub(line_starts[line]);
        let from = start.saturating_sub(PREVIEW_BEFORE);
        let to = (start + PREVIEW_AFTER).min(chars.len());
        let mut preview = String::new();
        if from > 0 {
            preview.push('…');
        }
        preview.extend(chars[from..to].iter());
        if to < chars.len() {
            preview.push('…');
        }
        found.push(Match {
            path: path.to_path_buf(),
            relative: relative.to_string(),
            line,
            column: start,
            preview: preview.trim_end().to_string(),
        });
        if found.len() >= budget {
            break;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn drain(task: &Task) -> (Vec<Match>, bool, bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut matches = Vec::new();
        while Instant::now() < deadline {
            while let Some(message) = task.try_recv() {
                match message {
                    Message::Matches(batch) => matches.extend(batch),
                    Message::Done { truncated } => return (matches, truncated, true),
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        (matches, false, false)
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-search-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("nested")).expect("dir");
        std::fs::write(dir.join("a.txt"), "first needle\nno match\nneedle again\n").expect("a");
        std::fs::write(dir.join("nested/b.txt"), "NEEDLE upper\n").expect("b");
        std::fs::write(dir.join("nested/skip.bin"), [0xff, 0xfe, 0x00]).expect("bin");
        dir
    }

    #[test]
    fn finds_lines_in_every_file_case_insensitively() {
        let dir = fixture("case");
        let files = vec!["a.txt".to_string(), "nested/b.txt".to_string()];
        let task =
            Task::spawn(&dir, files, "needle".to_string(), Options::default()).expect("task");
        let (matches, truncated, done) = drain(&task);
        assert!(done, "worker did not finish");
        assert!(!truncated);
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].relative, "a.txt");
        assert_eq!(matches[0].line, 0);
        assert_eq!(matches[0].column, 6);
        assert_eq!(matches[1].line, 2);
        assert_eq!(matches[2].relative, "nested/b.txt");
        assert_eq!(matches[2].column, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_uppercase_query_is_case_sensitive() {
        let dir = fixture("smart");
        let files = vec!["a.txt".to_string(), "nested/b.txt".to_string()];
        let options = Options {
            case_sensitive: true,
            ..Options::default()
        };
        let task = Task::spawn(&dir, files, "NEEDLE".to_string(), options).expect("task");
        let (matches, _, done) = drain(&task);
        assert!(done);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].relative, "nested/b.txt");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn binary_files_are_skipped() {
        let dir = fixture("binary");
        let task = Task::spawn(
            &dir,
            vec!["nested/skip.bin".to_string()],
            "needle".to_string(),
            Options::default(),
        )
        .expect("task");
        let (matches, _, done) = drain(&task);
        assert!(done);
        assert!(matches.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn results_stop_at_the_cap_and_report_truncation() {
        let dir = fixture("cap");
        let lines = "needle\n".repeat(MAX_RESULTS + 1);
        std::fs::write(dir.join("big.txt"), lines).expect("big");
        let task = Task::spawn(
            &dir,
            vec!["big.txt".to_string()],
            "needle".to_string(),
            Options::default(),
        )
        .expect("task");
        let (matches, truncated, done) = drain(&task);
        assert!(done);
        assert!(truncated);
        assert_eq!(matches.len(), MAX_RESULTS);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn long_line_previews_stay_bounded() {
        let dir = fixture("long-preview");
        let line = "a".repeat(128 * 1024);
        std::fs::write(dir.join("long.txt"), line).expect("long");
        let task = Task::spawn(
            &dir,
            vec!["long.txt".to_string()],
            "a".to_string(),
            Options::default(),
        )
        .expect("task");
        let (matches, truncated, done) = drain(&task);
        assert!(done);
        assert!(truncated);
        assert_eq!(matches.len(), MAX_RESULTS);
        assert!(
            matches
                .iter()
                .all(|found| found.preview.chars().count() <= PREVIEW_BEFORE + PREVIEW_AFTER + 2),
            "preview exceeded the bounded window"
        );
        let bytes: usize = matches.iter().map(|found| found.preview.len()).sum();
        assert!(bytes < 1_000_000, "preview bytes: {bytes}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unicode_columns_are_char_offsets() {
        let dir = fixture("unicode");
        std::fs::write(dir.join("unicode.txt"), "İ needle\n").expect("unicode");
        let task = Task::spawn(
            &dir,
            vec!["unicode.txt".to_string()],
            "needle".to_string(),
            Options::default(),
        )
        .expect("task");
        let (matches, _, done) = drain(&task);
        assert!(done);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].column, 2, "İ is one char but two bytes");
        assert!(matches[0].preview.contains("needle"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_query_is_rejected() {
        let dir = fixture("empty");
        assert!(
            Task::spawn(
                &dir,
                vec!["a.txt".to_string()],
                String::new(),
                Options::default(),
            )
            .is_err()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
