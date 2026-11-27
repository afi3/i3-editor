//! Git integration: repository discovery, line markers, status, diffs, and
//! staging. Everything shells out to `git`; no library dependency.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

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

impl StatusEntry {
    pub fn is_conflict(&self) -> bool {
        matches!(
            self.code.as_str(),
            "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU"
        )
    }
}

/// One hunk of a single-file unified diff, with enough context to apply it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@ ... @@` line.
    pub header: String,
    /// Header plus body, for the preview overlay.
    pub lines: Vec<String>,
    /// Complete patch (file header + this hunk) accepted by `git apply`.
    pub patch: String,
}

/// Repository root for `start`, when it is inside a work tree.
pub fn discover(binary: &str, start: &Path) -> Option<PathBuf> {
    let output = run(binary, start, &["rev-parse", "--show-toplevel"]).ok()?;
    let root = PathBuf::from(output.trim());
    root.is_dir().then_some(root)
}

/// Current branch, `detached at <sha>`, or `unborn` before the first commit.
pub fn branch(binary: &str, repo: &Path) -> Result<String, String> {
    if let Ok(branch) = run(binary, repo, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        return Ok(branch.trim().to_string());
    }
    match run(binary, repo, &["rev-parse", "--short", "HEAD"]) {
        Ok(sha) => Ok(format!("detached at {}", sha.trim())),
        Err(_) => Ok("unborn".to_string()),
    }
}

/// Local branches, one per line, prefixed with `*` for the current branch.
pub fn branches(binary: &str, repo: &Path) -> Result<Vec<String>, String> {
    let output = run(
        binary,
        repo,
        &["branch", "--format=%(HEAD)%(refname:short)"],
    )?;
    Ok(output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (marker, name) = line.split_at(1);
            format!("{} {}", if marker == "*" { "*" } else { " " }, name.trim())
        })
        .collect())
}

/// Recent commits as `hash  date  author  subject`.
pub fn log(binary: &str, repo: &Path, limit: usize) -> Result<Vec<String>, String> {
    let count = format!("-{limit}");
    let output = run(
        binary,
        repo,
        &[
            "log",
            "--no-color",
            "--date=short",
            "--pretty=format:%h  %ad  %an  %s",
            &count,
        ],
    )?;
    Ok(output.lines().map(str::to_string).collect())
}

/// Cheap check for a `.git` entry above `root`, so a non-repository workspace
/// does not spawn a marker worker every second.
pub fn has_repo(root: &Path) -> bool {
    if ancestor_has_git(root) {
        return true;
    }
    env_repo(root, std::env::var_os("GIT_DIR").as_deref())
}

fn ancestor_has_git(root: &Path) -> bool {
    let mut current = Some(root);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

/// A repository reached only through `GIT_DIR` still counts, but only when the
/// path it names holds `HEAD`. Trusting the variable alone made every workspace
/// spawn a marker worker for the whole session.
fn env_repo(root: &Path, value: Option<&std::ffi::OsStr>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let dir = PathBuf::from(value);
    let dir = if dir.is_absolute() {
        dir
    } else {
        root.join(dir)
    };
    dir.join("HEAD").is_file()
}

/// Per-line markers for every requested path in two `git` invocations: one
/// status for untracked files and one diff for the whole worktree. Paths
/// outside the repository, and paths without changes, map to no marks.
pub fn tree_marks(
    binary: &str,
    repo: &Path,
    paths: &[PathBuf],
) -> HashMap<PathBuf, Vec<Option<Change>>> {
    let wanted: Vec<(PathBuf, String)> = paths
        .iter()
        .filter_map(|path| relative_path(repo, path).map(|relative| (path.clone(), relative)))
        .collect();
    if wanted.is_empty() {
        return HashMap::new();
    }
    let status = run(
        binary,
        repo,
        &["-c", "core.quotePath=false", "status", "--porcelain"],
    )
    .unwrap_or_default();
    let untracked: HashSet<&str> = status
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .collect();
    let diff = run(
        binary,
        repo,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "HEAD",
            "--no-color",
            "--unified=0",
        ],
    )
    .unwrap_or_default();
    let mut marks = HashMap::new();
    for (path, relative) in wanted {
        let lines = if untracked.contains(relative.as_str()) {
            let count = std::fs::read_to_string(&path)
                .map(|text| text.lines().count())
                .unwrap_or(0)
                .max(1);
            vec![Some(Change::Added); count]
        } else {
            section(&diff, &relative)
                .map(|section| parse_marks(section, &path))
                .unwrap_or_default()
        };
        marks.insert(path, lines);
    }
    marks
}

/// Background marker pass for the documents on screen.
pub struct MarksTask {
    result: mpsc::Receiver<HashMap<PathBuf, Vec<Option<Change>>>>,
}

impl MarksTask {
    /// Discover the repository and compute markers off the UI thread; a missing
    /// repository yields an empty map.
    pub fn spawn(binary: String, root: PathBuf, paths: Vec<PathBuf>) -> Self {
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            let marks = match discover(&binary, &root) {
                Some(repo) => tree_marks(&binary, &repo, &paths),
                None => HashMap::new(),
            };
            let _ = sender.send(marks);
        });
        Self { result }
    }

    pub fn try_recv(&self) -> Option<HashMap<PathBuf, Vec<Option<Change>>>> {
        self.result.try_recv().ok()
    }
}

/// The `diff --git` section that belongs to one repository-relative path.
fn section<'a>(diff: &'a str, relative: &str) -> Option<&'a str> {
    let expected = format!("a/{relative} b/{relative}");
    let mut start = None;
    let mut end = diff.len();
    let mut offset = 0;
    for line in diff.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if start.is_some() {
                end = offset;
                break;
            }
            if rest.trim_end() == expected {
                start = Some(offset);
            }
        }
        offset += line.len();
    }
    start.map(|start| &diff[start..end])
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

/// Split a single-file diff into independently applicable hunks.
pub fn hunks(binary: &str, repo: &Path, relative: &str, staged: bool) -> Result<Vec<Hunk>, String> {
    let text = diff(binary, repo, relative, staged)?;
    Ok(parse_hunks(&text))
}

fn parse_hunks(text: &str) -> Vec<Hunk> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(first) = lines.iter().position(|line| line.starts_with("@@ ")) else {
        return Vec::new();
    };
    let header = lines[..first].join("\n");
    let mut hunks = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in &lines[first..] {
        if line.starts_with("@@ ") {
            if let Some((hunk_header, body)) = current.take() {
                hunks.push(make_hunk(&header, hunk_header, body));
            }
            current = Some(((*line).to_string(), vec![(*line).to_string()]));
        } else if let Some((_, body)) = current.as_mut() {
            body.push((*line).to_string());
        }
    }
    if let Some((hunk_header, body)) = current {
        hunks.push(make_hunk(&header, hunk_header, body));
    }
    hunks
}

fn make_hunk(header: &str, hunk_header: String, lines: Vec<String>) -> Hunk {
    let mut patch = String::new();
    if !header.is_empty() {
        patch.push_str(header);
        patch.push('\n');
    }
    patch.push_str(&lines.join("\n"));
    patch.push('\n');
    Hunk {
        header: hunk_header,
        lines,
        patch,
    }
}

/// Stage one hunk from an unstaged diff. Untracked files have no applicable
/// hunk context, so they fall back to staging the whole file.
pub fn stage_hunk(binary: &str, repo: &Path, relative: &str, patch: &str) -> Result<(), String> {
    if is_untracked(binary, repo, relative)? {
        return stage(binary, repo, relative);
    }
    apply_patch(binary, repo, patch, false)
}

/// Remove one hunk from the index using its staged patch.
pub fn unstage_hunk(binary: &str, repo: &Path, relative: &str, patch: &str) -> Result<(), String> {
    let _ = relative;
    apply_patch(binary, repo, patch, true)
}

fn apply_patch(binary: &str, repo: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    let mut args = vec!["apply", "--cached"];
    if reverse {
        args.push("--reverse");
    }
    let mut child = Command::new(binary)
        .args(&args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{binary}: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(patch.as_bytes())
            .map_err(|error| format!("git apply: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("git apply: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            stdout.trim()
        } else {
            detail
        };
        return Err(if detail.is_empty() {
            format!("git apply exited with {}", output.status)
        } else {
            detail.to_string()
        });
    }
    Ok(())
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

/// Fast-forward the current branch from its upstream. Refuses a dirty index or
/// worktree so a pull cannot create a merge/conflict state.
pub fn pull(binary: &str, repo: &Path) -> Result<String, String> {
    ensure_clean(binary, repo)?;
    ensure_upstream(binary, repo)?;
    let output = run(binary, repo, &["pull", "--ff-only", "--no-rebase"])?;
    Ok(summary(&output, "already up to date"))
}

/// Push the current branch to its configured upstream. Credential helpers and
/// SSH agents work; terminal prompts are disabled because the UI owns stdin.
pub fn push(binary: &str, repo: &Path) -> Result<String, String> {
    ensure_upstream(binary, repo)?;
    let output = run(binary, repo, &["push"])?;
    Ok(summary(&output, "pushed"))
}

fn ensure_clean(binary: &str, repo: &Path) -> Result<(), String> {
    let unstaged = run(binary, repo, &["diff", "--quiet"]).is_err();
    let staged = run(binary, repo, &["diff", "--cached", "--quiet"]).is_err();
    if unstaged || staged {
        return Err(
            "working tree has uncommitted changes; commit or stash them before pulling".to_string(),
        );
    }
    Ok(())
}

fn ensure_upstream(binary: &str, repo: &Path) -> Result<(), String> {
    run(
        binary,
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .map(|_| ())
    .map_err(|_| "no upstream branch; set one with: git push -u origin <branch>".to_string())
}

fn summary(output: &str, fallback: &str) -> String {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| fallback.to_string())
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
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
        let marks = tree_marks("git", &dir, std::slice::from_ref(&file));
        let marks = marks.get(&file).expect("marks");
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
        let marks = tree_marks("git", &dir, std::slice::from_ref(&file));
        assert_eq!(
            marks.get(&file),
            Some(&vec![Some(Change::Added), Some(Change::Added)])
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_pass_covers_every_requested_file() {
        let dir = repo("tree");
        let changed = dir.join("a.txt");
        let clean = dir.join("b.txt");
        let fresh = dir.join("c.txt");
        std::fs::write(&changed, "one\n").expect("seed");
        std::fs::write(&clean, "two\n").expect("seed");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        std::fs::write(&changed, "one\nmore\n").expect("edit");
        std::fs::write(&fresh, "new\n").expect("create");
        let unicode = dir.join("catatan-é.txt");
        std::fs::write(&unicode, "satu\n").expect("create");

        let marks = tree_marks(
            "git",
            &dir,
            &[
                changed.clone(),
                clean.clone(),
                fresh.clone(),
                unicode.clone(),
            ],
        );
        assert_eq!(marks[&changed][0], None);
        assert_eq!(marks[&changed][1], Some(Change::Added));
        assert!(marks[&clean].is_empty());
        assert_eq!(marks[&fresh], vec![Some(Change::Added)]);
        assert_eq!(marks[&unicode], vec![Some(Change::Added)]);
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
    fn one_hunk_can_be_staged_and_unstaged() {
        let dir = repo("hunks");
        let file = dir.join("a.txt");
        let base: String = (1..=20).map(|number| format!("line {number}\n")).collect();
        std::fs::write(&file, &base).expect("seed");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        let changed = base
            .replace("line 2\n", "line 2 changed\n")
            .replace("line 18\n", "line 18 changed\n");
        std::fs::write(&file, &changed).expect("edit");

        let hunks = hunks("git", &dir, "a.txt", false).expect("hunks");
        assert_eq!(hunks.len(), 2, "{hunks:#?}");
        stage_hunk("git", &dir, "a.txt", &hunks[1].patch).expect("stage hunk");
        let cached = diff("git", &dir, "a.txt", true).expect("cached diff");
        assert!(cached.contains("+line 18 changed"), "{cached}");
        assert!(!cached.contains("+line 2 changed"), "{cached}");
        let unstaged = diff("git", &dir, "a.txt", false).expect("unstaged diff");
        assert!(unstaged.contains("+line 2 changed"), "{unstaged}");
        assert!(!unstaged.contains("+line 18 changed"), "{unstaged}");

        unstage_hunk("git", &dir, "a.txt", &hunks[1].patch).expect("unstage hunk");
        assert!(
            diff("git", &dir, "a.txt", true)
                .expect("cached diff")
                .trim()
                .is_empty()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn branch_list_and_log_report_the_current_state() {
        let dir = repo("branchlog");
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        git(&dir, &["branch", "topic"]);

        let current = branch("git", &dir).expect("branch");
        assert!(!current.is_empty());
        let branches = branches("git", &dir).expect("branches");
        assert!(
            branches.iter().any(|line| line.contains(&current)),
            "{branches:?}"
        );
        assert!(
            branches.iter().any(|line| line.trim() == "topic"),
            "{branches:?}"
        );
        let log = log("git", &dir, 10).expect("log");
        assert!(log.iter().any(|line| line.contains("base")), "{log:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pull_and_push_round_trip_through_a_bare_remote() {
        let root = std::env::temp_dir().join(format!("i3-git-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&remote).expect("remote");
        git(&remote, &["init", "--bare", "-q"]);

        let one = root.join("one");
        std::fs::create_dir_all(&one).expect("one");
        git(&one, &["init", "-q"]);
        git(&one, &["config", "user.email", "test@example.com"]);
        git(&one, &["config", "user.name", "Test"]);
        std::fs::write(one.join("a.txt"), "one\n").expect("seed");
        git(&one, &["add", "."]);
        git(&one, &["commit", "-q", "-m", "base"]);
        let branch = branch("git", &one).expect("branch");
        git(
            &one,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git(&one, &["push", "-u", "origin", &branch]);

        let two = root.join("two");
        let clone = Command::new("git")
            .args([
                "clone",
                "-q",
                &remote.to_string_lossy(),
                &two.to_string_lossy(),
            ])
            .current_dir(&root)
            .status()
            .expect("clone");
        assert!(clone.success());
        git(&two, &["config", "user.email", "test@example.com"]);
        git(&two, &["config", "user.name", "Test"]);
        std::fs::write(two.join("a.txt"), "two\n").expect("edit");
        git(&two, &["add", "."]);
        git(&two, &["commit", "-q", "-m", "remote change"]);
        git(&two, &["push"]);

        pull("git", &one).expect("pull");
        assert_eq!(
            std::fs::read_to_string(one.join("a.txt")).expect("read"),
            "two\n"
        );

        std::fs::write(one.join("a.txt"), "three\n").expect("edit");
        git(&one, &["add", "."]);
        git(&one, &["commit", "-q", "-m", "local change"]);
        push("git", &one).expect("push");
        let log = Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(&remote)
            .output()
            .expect("remote log");
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "local change");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pull_refuses_a_dirty_worktree() {
        let dir = repo("pull-dirty");
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        std::fs::write(&file, "two\n").expect("edit");
        let error = pull("git", &dir).expect_err("dirty pull");
        assert!(error.contains("uncommitted changes"), "{error}");
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

    #[test]
    fn a_git_dir_env_only_counts_when_it_holds_head() {
        use std::ffi::OsStr;

        let dir = repo("env-git");
        assert!(env_repo(&dir, Some(OsStr::new(".git"))), "relative GIT_DIR");
        assert!(
            env_repo(&dir, Some(dir.join(".git").as_os_str())),
            "absolute GIT_DIR"
        );
        assert!(!env_repo(&dir, Some(OsStr::new("missing"))));
        assert!(!env_repo(&dir, None));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_directory_is_not_a_repository() {
        let dir = std::env::temp_dir().join(format!("i3-git-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        assert!(!has_repo(&dir));
        assert!(has_repo(&repo("marked")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
