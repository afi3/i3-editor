//! File operations that target the explorer selection or the active document.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::state::Focus;

use super::{App, same_path};

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
