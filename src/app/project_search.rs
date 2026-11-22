//! Find in Files: starting scans, draining results, and jumping to a match.

use crate::input::PromptKind;
use crate::project_search::{Message, Task};
use crate::state::{InputMode, ProjectSearch};

use super::App;

impl App {
    /// Cancel any running scan and search the cached project file list.
    pub(crate) fn start_project_search(&mut self, query: String) {
        self.cancel_project_search();
        self.state.project_search = Some(ProjectSearch {
            query: query.clone(),
            ..ProjectSearch::default()
        });
        if let InputMode::Prompt(prompt) = &mut self.state.mode
            && prompt.kind == PromptKind::FindInFiles
        {
            prompt.set_candidates(Vec::new());
        }
        if query.is_empty() {
            return;
        }
        let files = self.quick_open_candidates();
        let root = self.state.explorer.root.clone();
        self.project_task = Some(Task::spawn(&root, files, query));
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
        let mut finished = false;
        let mut truncated = false;
        for message in messages {
            match message {
                Message::Matches(batch) => {
                    let labels: Vec<String> = batch.iter().map(|found| found.label()).collect();
                    if let Some(search) = &mut self.state.project_search {
                        search.results.extend(batch);
                    }
                    if let InputMode::Prompt(prompt) = &mut self.state.mode
                        && prompt.kind == PromptKind::FindInFiles
                    {
                        prompt.candidates.extend(labels);
                        prompt.refilter();
                    }
                }
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
