//! Find, replace, and go to line.

use crate::error::{Error, Result};
use crate::state::{AppState, Focus, Search};

use super::App;

impl App {
    /// Recompute matches while the Find prompt is open and preview the first one.
    pub(crate) fn preview_search(&mut self, query: String) {
        if query.is_empty() {
            self.state.search = None;
            return;
        }
        let options = self.state.search_options;
        let Some(document_id) = self.state.active_document_id() else {
            return;
        };
        let Some(document) = self.state.documents.get(&document_id) else {
            return;
        };
        let text = document.rope.to_string();
        let matcher = match crate::search_options::Matcher::new(&query, options) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.state.search = None;
                self.notify(format!("invalid search: {error}"));
                return;
            }
        };
        let (matches, truncated) = matcher.find(&text);
        let total = matches.len();
        self.state.search = Some(Search {
            document: document_id,
            query: query.clone(),
            options,
            matches,
            index: 0,
            truncated,
        });
        if total == 0 {
            self.notify(format!("no match for {query}"));
            return;
        }
        self.show_match(0);
    }

    /// Enter in the Find prompt. Matches produced while typing are kept as they
    /// are, so an arrow-key selection survives the prompt closing.
    pub(crate) fn commit_search(&mut self, query: String) -> Result<()> {
        if query.is_empty() {
            self.state.search = None;
            self.notify("search cleared");
            return Ok(());
        }
        let active = self.state.active_document_id();
        let options = self.state.search_options;
        let live = self
            .state
            .search
            .as_ref()
            .filter(|search| {
                search.query == query
                    && Some(search.document) == active
                    && search.options == options
            })
            .map(|search| (search.index, search.matches.len()));
        match live {
            Some((_, 0)) => self.notify(format!("no match for {query}")),
            Some((index, total)) => {
                self.notify(format!("{query}: match {}/{total}", index + 1));
            }
            None => self.preview_search(query),
        }
        Ok(())
    }

    /// Up/Down in the Find prompt: step through the match list, wrapping.
    pub(crate) fn move_search_selection(&mut self, delta: isize) {
        let Some(search) = self.state.search.as_ref() else {
            self.notify("no active search");
            return;
        };
        let total = search.matches.len();
        if total == 0 {
            return;
        }
        let index = (search.index as isize + delta).rem_euclid(total as isize) as usize;
        self.show_match(index);
    }

    pub(crate) fn next_match(&mut self) -> Result<()> {
        let Some(search) = self.state.search.as_ref() else {
            self.notify("no active search");
            return Ok(());
        };
        if search.matches.is_empty() {
            return Ok(());
        }
        self.show_match(search.index + 1);
        Ok(())
    }

    /// Select `index` (wrapping) and move the pane cursor onto that match.
    fn show_match(&mut self, index: usize) {
        let Some(search) = self.state.search.as_ref() else {
            return;
        };
        let total = search.matches.len();
        if total == 0 {
            return;
        }
        let index = index % total;
        let range = search.matches[index].clone();
        let document = search.document;
        let query = search.query.clone();
        let line = self
            .state
            .documents
            .get(&document)
            .map(|document| document.line_of_char(range.start))
            .unwrap_or(0);
        if let Some(document) = self.state.documents.get_mut(&document) {
            document.unfold_line(line);
        }
        if let Some(search) = self.state.search.as_mut() {
            search.index = index;
        }
        if let Some(pane) = self.state.focused_pane_mut()
            && pane.active == Some(document)
        {
            pane.cursor = range.start;
            pane.anchor = Some(range.end);
            pane.target_col = None;
        }
        self.notify(format!("{query}: match {}/{total}", index + 1));
        self.ensure_cursor_visible();
    }

    pub(crate) fn goto_line(&mut self, line: usize) -> Result<()> {
        let Some(document) = self.state.active_document() else {
            return Err(Error::NoActiveDocument);
        };
        let line = line.min(document.line_count().saturating_sub(1));
        let char_idx = document.char_of_line(line);
        let id = document.id;
        if let Some(document) = self.state.documents.get_mut(&id) {
            document.unfold_line(line);
        }
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.cursor = char_idx;
            pane.anchor = None;
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
        Ok(())
    }

    pub(crate) fn replace_all(&mut self, needle: &str, replacement: &str) -> Result<()> {
        if needle.is_empty() {
            return Ok(());
        }
        let options = self.state.search_options;
        let matcher = match crate::search_options::Matcher::new(needle, options) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.notify(format!("invalid search: {error}"));
                return Ok(());
            }
        };
        let count;
        {
            let AppState {
                documents,
                panes,
                focus,
                ..
            } = &mut self.state;
            let pane_id = match focus {
                Focus::Pane(id) => *id,
                Focus::Explorer | Focus::Shell => return Ok(()),
            };
            let Some(pane) = panes.get_mut(&pane_id) else {
                return Ok(());
            };
            let Some(id) = pane.active else {
                return Ok(());
            };
            let Some(document) = documents.get_mut(&id) else {
                return Ok(());
            };
            let haystack = document.rope.to_string();
            let (matches, _) = matcher.find(&haystack);
            count = matches.len();
            if count == 0 {
                self.state.ui.message = "nothing replaced".to_string();
                return Ok(());
            }
            let new_text = matcher.replace_all(&haystack, replacement);
            let first = matches.first().map(|range| range.start).unwrap_or(0);
            let scroll = (pane.scroll_top, pane.scroll_left);
            document.replace(
                0..document.len_chars(),
                &new_text,
                pane.cursor,
                first,
                scroll,
            );
            pane.cursor = first.min(document.len_chars());
            pane.anchor = None;
            pane.target_col = None;
            pane.scroll_top = pane.scroll_top.min(document.line_count().saturating_sub(1));
        }
        self.state.search = None;
        if count == 0 {
            self.notify("nothing replaced");
        } else {
            self.notify(format!("replaced {count} occurrence(s)"));
        }
        self.ensure_cursor_visible();
        self.plugin_event("change");
        Ok(())
    }
}
