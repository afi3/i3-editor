//! Find/Replace in Files: starting scans, draining results, and writing the
//! selected file.
//!
//! The worker only reads. Replace in Files previews matches first and writes
//! one file per explicit confirmation, so a keystroke in the query field can
//! never touch the disk.

use std::path::Path;

use crate::editor::document::write_atomic;
use crate::input::PromptKind;
use crate::project_search::{FileMatches, Message, Task, group_by_file};
use crate::search_options::Matcher;
use crate::state::{AppState, InputMode, ProjectSearch};

use super::App;

impl App {
    /// Cancel any running scan and search the cached project file list.
    pub(crate) fn start_project_search(&mut self, query: String) {
        self.cancel_project_search();
        let options = self.state.search_options;
        self.state.project_search = Some(ProjectSearch {
            query: query.clone(),
            options,
            ..ProjectSearch::default()
        });
        if let InputMode::Prompt(prompt) = &mut self.state.mode
            && matches!(
                prompt.kind,
                PromptKind::FindInFiles | PromptKind::ReplaceInFiles
            )
        {
            prompt.set_candidates(Vec::new());
        }
        if query.is_empty() {
            return;
        }
        let files = self.quick_open_candidates();
        let root = self.state.explorer.root.clone();
        match Task::spawn(&root, files, query, options) {
            Ok(task) => self.project_task = Some(task),
            Err(error) => {
                self.state.project_search = None;
                self.notify(format!("invalid search: {error}"));
            }
        }
    }

    pub(crate) fn cancel_project_search(&mut self) {
        if let Some(task) = self.project_task.take() {
            task.cancel();
        }
    }

    /// Stop the scan but keep the query, so reopening the prompt prefills it.
    pub(crate) fn stop_project_search(&mut self) {
        self.cancel_project_search();
        if let Some(search) = &mut self.state.project_search {
            search.results.clear();
            search.truncated = false;
        }
    }

    /// Append streamed batches to the prompt and report when the scan ends.
    pub(crate) fn pump_project_search(&mut self) {
        let Some(task) = &self.project_task else {
            return;
        };
        let mut messages = Vec::new();
        while let Some(message) = task.try_recv() {
            messages.push(message);
        }
        if messages.is_empty() {
            return;
        }
        self.dirty = true;
        let kind = match &self.state.mode {
            InputMode::Prompt(prompt) => Some(prompt.kind),
            _ => None,
        };
        let mut finished = false;
        let mut truncated = false;
        for message in messages {
            match message {
                Message::Matches(batch) => self.append_project_matches(batch, kind),
                Message::Done { truncated: hit } => {
                    finished = true;
                    truncated = hit;
                }
            }
        }
        if !finished {
            return;
        }
        self.project_task = None;
        let Some(search) = &mut self.state.project_search else {
            return;
        };
        search.truncated = truncated;
        let query = search.query.clone();
        let count = search.results.len();
        if count == 0 {
            self.notify(format!("no matches for {query}"));
        } else if truncated {
            self.notify(format!("{query}: first {count} matches; refine the query"));
        } else {
            self.notify(format!("{query}: {count} matches"));
        }
    }

    /// Append one worker batch to the results and the open prompt: match rows
    /// for Find in Files, one grouped row per file for Replace in Files.
    fn append_project_matches(
        &mut self,
        batch: Vec<crate::project_search::Match>,
        kind: Option<PromptKind>,
    ) {
        let find_labels: Vec<String> = if kind == Some(PromptKind::FindInFiles) {
            batch.iter().map(|found| found.label()).collect()
        } else {
            Vec::new()
        };
        if let Some(search) = &mut self.state.project_search {
            search.results.extend(batch);
        }
        let AppState {
            mode,
            project_search,
            ..
        } = &mut self.state;
        if let InputMode::Prompt(prompt) = mode {
            match prompt.kind {
                PromptKind::FindInFiles => {
                    prompt.candidates.extend(find_labels);
                    prompt.refilter();
                }
                PromptKind::ReplaceInFiles => {
                    if let Some(search) = project_search.as_ref() {
                        prompt.candidates = replace_labels(search);
                    }
                    prompt.refilter();
                }
                _ => {}
            }
        }
    }

    /// Replace matches in the selected file only, then rescan. Called from the
    /// Replace in Files prompt; nothing is written until this runs.
    pub(crate) fn replace_selected_project_file(&mut self) -> crate::error::Result<()> {
        let (query, replacement, index) = {
            let InputMode::Prompt(prompt) = &self.state.mode else {
                return Ok(());
            };
            if prompt.kind != PromptKind::ReplaceInFiles {
                return Ok(());
            }
            let index = prompt.filtered.get(prompt.selected).copied().unwrap_or(0);
            (prompt.fields[0].clone(), prompt.fields[1].clone(), index)
        };
        if query.is_empty() {
            self.notify("nothing to replace");
            return Ok(());
        }
        let options = self.state.search_options;
        let matcher = match Matcher::new(&query, options) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.notify(format!("invalid search: {error}"));
                return Ok(());
            }
        };
        let group = self
            .state
            .project_search
            .as_ref()
            .map(|search| group_by_file(&search.results))
            .and_then(|groups| groups.get(index).cloned());
        let Some(group) = group else {
            self.notify("no file selected");
            return Ok(());
        };
        let path = self.state.explorer.root.join(&group.relative);
        let open = self
            .state
            .documents
            .iter()
            .find(|(_, document)| {
                document
                    .path
                    .as_deref()
                    .is_some_and(|open| same_file(open, &path))
            })
            .map(|(id, _)| *id);
        let count;
        if let Some(id) = open {
            let (cursor, scroll) = self
                .state
                .focused_pane()
                .filter(|pane| pane.active == Some(id))
                .map(|pane| (pane.cursor, (pane.scroll_top, pane.scroll_left)))
                .unwrap_or((0, (0, 0)));
            let document = self
                .state
                .documents
                .get_mut(&id)
                .expect("document looked up above");
            if document.read_only {
                self.notify(format!("{} is read-only", group.relative));
                return Ok(());
            }
            if document.dirty {
                self.notify(format!(
                    "{} has unsaved changes; save it first",
                    group.relative
                ));
                return Ok(());
            }
            let text = document.rope().to_string();
            count = matcher.replacement_count(&text);
            if count == 0 {
                self.notify(format!("{}: nothing replaced", group.relative));
                return Ok(());
            }
            let new_text = matcher.replace_all(&text, &replacement);
            document.replace(0..document.len_chars(), &new_text, cursor, 0, scroll);
            document.save()?;
            let len = document.len_chars();
            for pane in self.state.panes.values_mut() {
                if pane.tabs.contains(&id) {
                    pane.cursor = pane.cursor.min(len);
                    pane.anchor = None;
                    pane.target_col = None;
                }
            }
        } else {
            let bytes = std::fs::read(&path)
                .map_err(|error| crate::error::Error::io("read", &path, error))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| crate::error::Error::InvalidUtf8 { path: path.clone() })?;
            count = matcher.replacement_count(&text);
            if count == 0 {
                self.notify(format!("{}: nothing replaced", group.relative));
                return Ok(());
            }
            let new_text = matcher.replace_all(&text, &replacement);
            write_atomic(&path, new_text.as_bytes())
                .map_err(|error| crate::error::Error::io("write", &path, error))?;
        }
        self.notify(format!("{}: replaced {count}", group.relative));
        self.start_project_search(query);
        Ok(())
    }

    /// Open the match's file and put the cursor on it.
    pub(crate) fn jump_to_project_match(
        &mut self,
        found: crate::project_search::Match,
    ) -> crate::error::Result<()> {
        self.open_document(found.path.clone())?;
        let Some(id) = self.state.active_document_id() else {
            return Ok(());
        };
        let Some(document) = self.state.documents.get(&id) else {
            return Ok(());
        };
        let line_start = document.char_of_line(found.line);
        let line_end = if found.line + 1 < document.line_count() {
            document.char_of_line(found.line + 1)
        } else {
            document.len_chars()
        };
        let offset = (line_start + found.column).min(line_end);
        if let Some(document) = self.state.documents.get_mut(&id) {
            document.unfold_line(found.line);
        }
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.cursor = offset;
            pane.anchor = None;
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
        self.notify(format!("{}:{}", found.relative, found.line + 1));
        Ok(())
    }
}

fn replace_labels(search: &ProjectSearch) -> Vec<String> {
    group_by_file(&search.results)
        .into_iter()
        .map(|group: FileMatches| {
            format!(
                "{}  ({} match{})",
                group.relative,
                group.count,
                if group.count == 1 { "" } else { "es" }
            )
        })
        .collect()
}

fn same_file(open: &Path, target: &Path) -> bool {
    if open == target {
        return true;
    }
    matches!(
        (std::fs::canonicalize(open), std::fs::canonicalize(target)),
        (Ok(open), Ok(target)) if open == target
    )
}
