//! Git commands: markers, diff view, staging, and commit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::git::{self, StatusEntry};
use crate::input::PromptKind;
use crate::state::{DiffView, DocumentId};

use super::App;

const REFRESH: Duration = Duration::from_secs(1);

impl App {
    pub(crate) fn refresh_git_if_due(&mut self) {
        if self.last_git.elapsed() < REFRESH {
            return;
        }
        self.last_git = Instant::now();
        self.refresh_git_marks();
    }

    /// Recompute per-line markers for documents shown in panes.
    pub(crate) fn refresh_git_marks(&mut self) {
        let binary = self.state.config.git.binary.clone();
        let root = self.state.explorer.root.clone();
        let Some(repo) = git::discover(&binary, &root) else {
            self.state.git_marks.clear();
            return;
        };
        let mut ids = std::collections::HashSet::new();
        for pane in self.state.panes.values() {
            ids.extend(pane.tabs.iter().copied());
        }
        let shown: Vec<(DocumentId, PathBuf)> = ids
            .into_iter()
            .filter_map(|id| {
                self.state
                    .documents
                    .get(&id)
                    .and_then(|document| document.path.clone().map(|path| (id, path)))
            })
            .collect();
        let mut marks = HashMap::new();
        for (id, path) in shown {
            if let Ok(lines) = git::line_marks(&binary, &repo, &path) {
                marks.insert(id, lines);
            }
        }
        self.state.git_marks = marks;
    }

    fn active_git_file(&self) -> Result<(PathBuf, String)> {
        let document = self
            .state
            .active_document()
            .ok_or(Error::NoActiveDocument)?;
        let path = document
            .path
            .as_deref()
            .ok_or_else(|| Error::Git("document has no path".to_string()))?;
        let binary = &self.state.config.git.binary;
        let repo = git::discover(binary, &self.state.explorer.root)
            .ok_or_else(|| Error::Git("not a git repository".to_string()))?;
        let relative = relative(&repo, path)
            .ok_or_else(|| Error::Git("file is outside the repository".to_string()))?;
        Ok((repo, relative))
    }

    pub(crate) fn git_diff(&mut self) -> Result<()> {
        let (repo, relative) = self.active_git_file()?;
        let binary = self.state.config.git.binary.clone();
        let mut text = git::diff(&binary, &repo, &relative, false).map_err(Error::Git)?;
        if text.trim().is_empty() {
            text = git::diff(&binary, &repo, &relative, true).map_err(Error::Git)?;
        }
        self.show_git_diff(format!("Diff: {relative}"), &text);
        Ok(())
    }

    pub(crate) fn git_status(&mut self) -> Result<()> {
        self.open_prompt(PromptKind::GitStatus);
        Ok(())
    }

    pub(crate) fn git_stage(&mut self) -> Result<()> {
        let (repo, relative) = self.active_git_file()?;
        let binary = self.state.config.git.binary.clone();
        git::stage(&binary, &repo, &relative).map_err(Error::Git)?;
        self.notify(format!("staged {relative}"));
        self.refresh_git_marks();
        Ok(())
    }

    pub(crate) fn git_hunk_picker(&mut self, stage: bool) -> Result<()> {
        let (repo, relative) = self.active_git_file()?;
        let binary = self.state.config.git.binary.clone();
        let hunks = git::hunks(&binary, &repo, &relative, !stage).map_err(Error::Git)?;
        if hunks.is_empty() {
            self.notify(if stage {
                "no unstaged changes"
            } else {
                "no staged changes"
            });
            return Ok(());
        }
        self.state.completions = None;
        self.state.hover = None;
        self.state.help = None;
        self.state.git_diff = None;
        self.state.hunks = Some(crate::state::HunkPicker {
            title: format!(
                " {} hunk: {relative} ",
                if stage { "Stage" } else { "Unstage" }
            ),
            relative,
            stage,
            hunks,
            selected: 0,
            scroll: 0,
        });
        Ok(())
    }

    pub(crate) fn apply_selected_hunk(&mut self) -> Result<()> {
        let Some(picker) = self.state.hunks.as_ref() else {
            return Ok(());
        };
        let Some(hunk) = picker.hunks.get(picker.selected).cloned() else {
            return Ok(());
        };
        let stage = picker.stage;
        let relative = picker.relative.clone();
        let index = picker.selected + 1;
        let total = picker.hunks.len();
        let binary = self.state.config.git.binary.clone();
        let repo = git::discover(&binary, &self.state.explorer.root)
            .ok_or_else(|| Error::Git("not a git repository".to_string()))?;
        let result = if stage {
            git::stage_hunk(&binary, &repo, &relative, &hunk.patch)
        } else {
            git::unstage_hunk(&binary, &repo, &relative, &hunk.patch)
        };
        match result {
            Ok(()) => {
                self.state.hunks = None;
                self.notify(format!(
                    "{} hunk {index}/{total} of {relative}",
                    if stage { "staged" } else { "unstaged" }
                ));
                self.refresh_git_marks();
            }
            Err(error) => self.notify(format!("git: {error}")),
        }
        Ok(())
    }

    /// Keys while the hunk picker is open. Enter applies the selected hunk;
    /// Esc/q closes without changing the index.
    pub(crate) fn handle_hunk_key(&mut self, combo: crate::input::KeyCombo) -> bool {
        use ratatui::crossterm::event::KeyCode;

        let mut apply = false;
        {
            let Some(picker) = self.state.hunks.as_mut() else {
                return false;
            };
            let last_hunk = picker.hunks.len().saturating_sub(1);
            let body = picker
                .hunks
                .get(picker.selected)
                .map_or(0, |hunk| hunk.lines.len().saturating_sub(1));
            match combo.code {
                KeyCode::Up => {
                    picker.selected = picker.selected.saturating_sub(1);
                    picker.scroll = 0;
                }
                KeyCode::Down => {
                    picker.selected = (picker.selected + 1).min(last_hunk);
                    picker.scroll = 0;
                }
                KeyCode::PageUp => picker.scroll = picker.scroll.saturating_sub(10),
                KeyCode::PageDown => picker.scroll = (picker.scroll + 10).min(body),
                KeyCode::Home => picker.scroll = 0,
                KeyCode::End => picker.scroll = body,
                KeyCode::Enter => apply = true,
                KeyCode::Esc | KeyCode::Char('q') => self.state.hunks = None,
                _ => {}
            }
        }
        if apply {
            let result = self.apply_selected_hunk();
            self.finish(result);
        }
        true
    }

    pub(crate) fn git_unstage(&mut self) -> Result<()> {
        let (repo, relative) = self.active_git_file()?;
        let binary = self.state.config.git.binary.clone();
        git::unstage(&binary, &repo, &relative).map_err(Error::Git)?;
        self.notify(format!("unstaged {relative}"));
        self.refresh_git_marks();
        Ok(())
    }

    pub(crate) fn git_commit_prompt(&mut self) -> Result<()> {
        self.open_prompt(PromptKind::CommitMessage);
        Ok(())
    }

    pub(crate) fn submit_commit(&mut self, message: &str) -> Result<()> {
        let message = message.trim();
        if message.is_empty() {
            self.notify("empty commit message");
            return Ok(());
        }
        let binary = self.state.config.git.binary.clone();
        let repo = git::discover(&binary, &self.state.explorer.root)
            .ok_or_else(|| Error::Git("not a git repository".to_string()))?;
        let output = git::commit(&binary, &repo, message).map_err(Error::Git)?;
        let summary = output
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("committed");
        self.notify(summary.trim().to_string());
        self.refresh_git_marks();
        Ok(())
    }

    /// Open the diff for a file picked in the status prompt.
    pub(crate) fn open_git_entry(&mut self, entry: StatusEntry) -> Result<()> {
        let binary = self.state.config.git.binary.clone();
        let repo = git::discover(&binary, &self.state.explorer.root)
            .ok_or_else(|| Error::Git("not a git repository".to_string()))?;
        let staged = entry.staged && !entry.unstaged;
        let text = git::diff(&binary, &repo, &entry.relative, staged).map_err(Error::Git)?;
        self.show_git_diff(format!("Diff: {}", entry.relative), &text);
        Ok(())
    }

    fn show_git_diff(&mut self, title: String, text: &str) {
        if text.trim().is_empty() {
            self.notify("no changes to show");
            return;
        }
        self.state.completions = None;
        self.state.hover = None;
        self.state.git_diff = Some(DiffView {
            title: format!(" {title} (Up/Down scroll, Esc closes) "),
            lines: text.lines().map(str::to_string).collect(),
            scroll: 0,
        });
    }

    /// Keys while the read-only diff overlay is open: scroll, Esc/q closes, and
    /// every other key is consumed so the view stays read-only.
    pub(crate) fn handle_diff_key(&mut self, combo: crate::input::KeyCombo) -> bool {
        use ratatui::crossterm::event::KeyCode;

        let Some(view) = &mut self.state.git_diff else {
            return false;
        };
        let last = view.lines.len().saturating_sub(1);
        match combo.code {
            KeyCode::Up => view.scroll = view.scroll.saturating_sub(1),
            KeyCode::Down => view.scroll = (view.scroll + 1).min(last),
            KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(20),
            KeyCode::PageDown => view.scroll = (view.scroll + 20).min(last),
            KeyCode::Home => view.scroll = 0,
            KeyCode::End => view.scroll = last,
            KeyCode::Esc | KeyCode::Char('q') => self.state.git_diff = None,
            _ => {}
        }
        true
    }
}

fn relative(repo: &Path, path: &Path) -> Option<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    path.strip_prefix(repo)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}
