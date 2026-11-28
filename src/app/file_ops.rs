//! File operations that target the explorer selection or the active document.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::state::Focus;

use super::{App, same_path};

/// The canonical form of a path whose tail may no longer exist: the nearest
/// existing ancestor is resolved and the rest is re-appended. A rename makes
/// the entry itself, and everything below it, unresolvable.
fn resolve_entry(path: &Path) -> PathBuf {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut resolved = canonical;
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        let Some(name) = current.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => break,
        }
    }
    path.to_path_buf()
}

/// `base.join("")` leaves a trailing separator; a buffer path must not have one.
fn join_rest(base: &Path, rest: &Path) -> PathBuf {
    if rest.as_os_str().is_empty() {
        base.to_path_buf()
    } else {
        base.join(rest)
    }
}

impl App {
    /// Path acted on by file operations. The explorer drives when it has focus;
    /// otherwise the active document wins, with the explorer row as a fallback.
    fn file_operation_path(&self) -> Option<PathBuf> {
        if matches!(self.state.focus, Focus::Explorer) {
            return self
                .state
                .explorer
                .selected_row()
                .map(|row| row.path.clone());
        }
        self.state
            .active_document()
            .and_then(|document| document.path.clone())
            .or_else(|| {
                self.state
                    .explorer
                    .selected_row()
                    .map(|row| row.path.clone())
            })
    }

    pub(crate) fn duplicate_file(&mut self) -> Result<()> {
        let Some(path) = self.file_operation_path() else {
            self.notify("no file selected");
            return Ok(());
        };
        if path.is_dir() {
            self.notify("select a file to duplicate");
            return Ok(());
        }
        if self.state.documents.values().any(|document| {
            document.dirty
                && document
                    .path
                    .as_deref()
                    .is_some_and(|open| same_path(open, &path))
        }) {
            self.notify("save the file before duplicating");
            return Ok(());
        }
        let target = self.state.explorer.duplicate(&path)?;
        self.notify(format!("duplicated to {}", target.display()));
        Ok(())
    }

    /// Point open buffers at a renamed file or directory. Without this a save
    /// would write the old name back and leave the renamed file untouched.
    pub(crate) fn retarget_documents(&mut self, from: &Path, to: &Path) {
        let from_resolved = resolve_entry(from);
        let to_resolved = resolve_entry(to);
        let mut moved = 0;
        for document in self.state.documents.values_mut() {
            let Some(path) = document.path.as_deref().map(Path::to_path_buf) else {
                continue;
            };
            // Prefer the path as the buffer stored it, so the style stays
            // consistent; fall back to the canonical comparison when the two
            // differ (a symlinked temp root, for instance).
            let target = match path.strip_prefix(from) {
                Ok(rest) => Some(join_rest(to, rest)),
                Err(_) => resolve_entry(&path)
                    .strip_prefix(&from_resolved)
                    .ok()
                    .map(|rest| join_rest(&to_resolved, rest)),
            };
            let Some(target) = target else {
                continue;
            };
            // The recorded disk stamp stays: a rename preserves mtime and size,
            // so an external change the user was warned about must remain
            // flagged instead of being reset by the new path.
            document.path = Some(target);
            moved += 1;
        }
        if moved > 0 {
            self.refresh_git_marks();
        }
    }

    /// Keep other workspaces pointing at a renamed directory. Without this a
    /// nested workspace keeps a dangling root and a watcher on the old path.
    pub(crate) fn retarget_workspaces(&mut self, from: &Path, to: &Path) {
        let from_resolved = resolve_entry(from);
        let to_resolved = resolve_entry(to);
        let active = self.state.workspaces.active().id;
        let mut changed: Vec<(crate::state::WorkspaceId, PathBuf)> = Vec::new();
        for workspace in &mut self.state.workspaces.workspaces {
            if let Ok(rest) = resolve_entry(&workspace.root).strip_prefix(&from_resolved) {
                workspace.root = join_rest(&to_resolved, rest);
                changed.push((workspace.id, workspace.root.clone()));
            }
            if let Ok(rest) = resolve_entry(&workspace.terminal_dir).strip_prefix(&from_resolved) {
                workspace.terminal_dir = join_rest(&to_resolved, rest);
            }
        }
        let active_moved = changed.iter().any(|(id, _)| *id == active);
        for (id, root) in changed {
            self.replace_watcher(id, &root);
        }
        if active_moved {
            self.state.explorer.root = self.state.workspaces.active().root.clone();
        }
    }

    pub(crate) fn copy_path(&mut self) -> Result<()> {
        let Some(path) = self.file_operation_path() else {
            self.notify("no file selected");
            return Ok(());
        };
        let text = super::absolutize(&path)?.to_string_lossy().into_owned();
        self.state.clipboard = text.clone();
        match crate::clipboard::copy_osc52(&text) {
            Some(reason) => {
                self.notify(format!("copied path to internal clipboard only ({reason})"))
            }
            None => self.notify(format!("copied path: {text}")),
        }
        Ok(())
    }

    pub(crate) fn reveal_in_terminal(&mut self) -> Result<()> {
        let Some(path) = self.file_operation_path() else {
            self.notify("no file selected");
            return Ok(());
        };
        let cwd = if path.is_dir() {
            path
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .ok_or(Error::InvalidPath(path))?
        };
        self.open_terminal_at(cwd);
        Ok(())
    }
}
