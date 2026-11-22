//! Git integration: repository discovery, line markers, status, diffs, and
//! staging. Everything shells out to `git`; no library dependency.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub relative: String,
    /// Two-letter porcelain code (`M `, ` M`, `??`, ...).
    pub code: String,
    pub staged: bool,
    pub unstaged: bool,
}

/// Repository root for `start`, when it is inside a work tree.
pub fn discover(binary: &str, start: &Path) -> Option<PathBuf> {
    let output = run(binary, start, &["rev-parse", "--show-toplevel"]).ok()?;
    let root = PathBuf::from(output.trim());
    root.is_dir().then_some(root)
}

/// Per-line markers for `path`: compare the worktree (including the index)
/// against `HEAD`. Untracked files are reported as entirely added.
pub fn line_marks(binary: &str, repo: &Path, path: &Path) -> Result<Vec<Option<Change>>, String> {
    let Some(relative) = relative_path(repo, path) else {
        return Ok(Vec::new());
    };
    if is_untracked(binary, repo, &relative)? {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let lines = text.lines().count().max(1);
        return Ok(vec![Some(Change::Added); lines]);
    }
    let output = run(
        binary,
        repo,
        &["diff", "HEAD", "--no-color", "--unified=0", "--", &relative],
    )?;
    Ok(parse_marks(&output, path))
}

fn parse_marks(diff: &str, path: &Path) -> Vec<Option<Change>> {
    let mut marks: Vec<Option<Change>> = Vec::new();
    let line_count = std::fs::read_to_string(path)
        .map(|text| text.lines().count())
        .unwrap_or(0);
    for line in diff.lines() {
        let Some(rest) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some((old, new)) =
            rest.split_once(" @@")
                .map(|(ranges, _)| ranges)
                .and_then(|ranges| {
                    let mut parts = ranges.split(' ');
                    let old = parts.next()?;
                    let new = parts.next()?;
                    Some((parse_range(old)?, parse_range(new)?))
                })
        else {
            continue;
        };
        apply_hunk(&mut marks, old.1, new.0, new.1, line_count);
    }
    marks
}

fn apply_hunk(
    marks: &mut Vec<Option<Change>>,
    old_len: usize,
    new_start: usize,
    new_len: usize,
    line_count: usize,
) {
    let ensure = |marks: &mut Vec<Option<Change>>, index: usize| {
        if marks.len() <= index {
            marks.resize(index + 1, None);
        }
    };
    let index = new_start.saturating_sub(1);
    if new_len == 0 && old_len > 0 {
        let at = index.min(line_count.saturating_sub(1));
        ensure(marks, at);
        marks[at] = Some(Change::Deleted);
        return;
    }
    for offset in 0..new_len {
        let at = index + offset;
        ensure(marks, at);
        marks[at] = Some(if old_len == 0 {
            Change::Added
        } else {
            Change::Modified
        });
    }
    if old_len > new_len {
        let at = (index + new_len).min(line_count.saturating_sub(1));
        ensure(marks, at);
        if marks[at].is_none() {
            marks[at] = Some(Change::Deleted);
        }
    }
}

fn parse_range(text: &str) -> Option<(usize, usize)> {
    let text = text.trim_start_matches(['-', '+']);
    let (start, len) = match text.split_once(',') {
        Some((start, len)) => (start.parse().ok()?, len.parse().ok()?),
        None => (text.parse().ok()?, 1),
    };
    Some((start, len))
}

fn is_untracked(binary: &str, repo: &Path, relative: &str) -> Result<bool, String> {
    let output = run(binary, repo, &["status", "--porcelain", "--", relative])?;
    Ok(output.trim_start().starts_with("??"))
}

/// Status entries, staged and unstaged. Renames report the new path.
pub fn status(binary: &str, repo: &Path) -> Result<Vec<StatusEntry>, String> {
    let output = run(binary, repo, &["status", "--porcelain"])?;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for line in output.lines() {
        if line.len() < 4 {
            continue;
        }
        let code = line[..2].to_string();
        let rest = line[3..].trim();
        if rest.is_empty() {
            continue;
        }
        let relative = rest
            .rsplit_once(" -> ")
            .map(|(_, new)| new.to_string())
            .unwrap_or_else(|| rest.to_string());
        if !seen.insert(relative.clone()) {
            continue;
        }
        let staged = !matches!(code.as_bytes()[0], b' ' | b'?');
        let unstaged = code.starts_with("??") || !matches!(code.as_bytes()[1], b' ');
        entries.push(StatusEntry {
            path: repo.join(&relative),
            relative,
            code,
            staged,
            unstaged,
        });
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(entries)
}

/// Unified diff for one file. `staged` reads the index; an untracked file gets
/// a synthetic all-added diff so the preview still works.
pub fn diff(binary: &str, repo: &Path, relative: &str, staged: bool) -> Result<String, String> {
    let args = if staged {
        vec!["diff", "--cached", "--no-color", "--", relative]
    } else {
        vec!["diff", "--no-color", "--", relative]
    };
    let output = run(binary, repo, &args)?;
    if !output.trim().is_empty() {
        return Ok(output);
    }
    let path = repo.join(relative);
    if !path.is_file() {
        return Ok(String::new());
    }
    let tracked = run(
        binary,
        repo,
        &["ls-files", "--error-unmatch", "--", relative],
    )
    .is_ok();
    if tracked {
        return Ok(String::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut out = format!("--- /dev/null\n+++ b/{relative}\n");
    for line in text.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

pub fn stage(binary: &str, repo: &Path, relative: &str) -> Result<(), String> {
    run(binary, repo, &["add", "--", relative]).map(|_| ())
}

pub fn unstage(binary: &str, repo: &Path, relative: &str) -> Result<(), String> {
    run(binary, repo, &["restore", "--staged", "--", relative]).map(|_| ())
}

pub fn commit(binary: &str, repo: &Path, message: &str) -> Result<String, String> {
    run(binary, repo, &["commit", "-m", message])
}

fn relative_path(repo: &Path, path: &Path) -> Option<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    path.strip_prefix(root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn run(binary: &str, cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("{binary}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stdout = stdout.trim();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if detail.is_empty() {
            format!("git exited with {}", output.status)
        } else {
            detail.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-git-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn modified_and_added_lines_are_marked() {
        let dir = repo("marks");
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").expect("seed");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);

        std::fs::write(&file, "one\nTWO\nthree\nfour\n").expect("edit");
        let marks = line_marks("git", &dir, &file).expect("marks");
        assert_eq!(marks[1], Some(Change::Modified));
        assert_eq!(marks[3], Some(Change::Added));
        assert_eq!(marks[0], None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn untracked_files_are_all_added() {
        let dir = repo("untracked");
        let file = dir.join("new.txt");
        std::fs::write(&file, "a\nb\n").expect("write");
        let marks = line_marks("git", &dir, &file).expect("marks");
        assert_eq!(marks, vec![Some(Change::Added), Some(Change::Added)]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn staged_and_unstaged_status_and_stage_round_trip() {
        let dir = repo("status");
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        std::fs::write(&file, "two\n").expect("edit");

        let entries = status("git", &dir).expect("status");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].code, " M");
        assert!(entries[0].unstaged);

        stage("git", &dir, "a.txt").expect("stage");
        let entries = status("git", &dir).expect("status");
        assert!(entries[0].staged);
        let cached = diff("git", &dir, "a.txt", true).expect("diff");
        assert!(cached.contains("+two"), "{cached}");

        unstage("git", &dir, "a.txt").expect("unstage");
        let entries = status("git", &dir).expect("status");
        assert!(!entries[0].staged);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_reports_git_errors() {
        let dir = repo("commit");
        let error = commit("git", &dir, "nothing").expect_err("empty commit");
        assert!(
            error.contains("nothing") || error.contains("Changes"),
            "{error}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
