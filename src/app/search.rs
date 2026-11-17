//! Find, replace, and go to line.

use crate::error::{Error, Result};
use crate::state::{AppState, Focus, Search};

use super::App;

impl App {
    pub(crate) fn start_search(&mut self, query: String) -> Result<()> {
        if query.is_empty() {
            self.state.search = None;
            self.notify("search cleared");
            return Ok(());
        }
        let Some(document) = self.state.active_document() else {
            return Err(Error::NoActiveDocument);
        };
        let haystack = document.rope.to_string();
        let matches = match_positions(&haystack, &query);
        let total = matches.len();
        self.state.search = Some(Search {
            query: query.clone(),
            matches,
            index: 0,
        });
        if total == 0 {
            self.notify(format!("no match for {query}"));
            return Ok(());
        }
        self.next_match()
    }

    pub(crate) fn next_match(&mut self) -> Result<()> {
        let Some(search) = self.state.search.as_ref() else {
            self.notify("no active search");
            return Ok(());
        };
        if search.matches.is_empty() {
            return Ok(());
        }
        let cursor = self
            .state
            .focused_pane()
            .map(|pane| pane.cursor)
            .unwrap_or(0);
        let index = search
            .matches
            .iter()
            .position(|position| *position >= cursor)
            .unwrap_or(0);
        let start = search.matches[index];
        let end = start + search.query.chars().count();
        let total = search.matches.len();
        let query = search.query.clone();
        if let Some(search) = self.state.search.as_mut() {
            search.index = index;
        }
        if let Some(pane) = self.state.focused_pane_mut() {
            pane.cursor = start;
            pane.anchor = Some(end);
            pane.target_col = None;
        }
        self.notify(format!("{query}: match {}/{total}", index + 1));
        self.ensure_cursor_visible();
        Ok(())
    }

    pub(crate) fn goto_line(&mut self, line: usize) -> Result<()> {
        let Some(document) = self.state.active_document() else {
            return Err(Error::NoActiveDocument);
        };
        let line = line.min(document.line_count().saturating_sub(1));
        let char_idx = document.char_of_line(line);
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
                Focus::Explorer => return Ok(()),
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
            let positions = match_positions(&haystack, needle);
            let scroll = (pane.scroll_top, pane.scroll_left);
            let cursor_before = pane.cursor;
            let needle_len = needle.chars().count();
            for position in positions.iter().rev() {
                document.remove(*position..*position + needle_len, cursor_before, scroll);
                document.insert(*position, replacement, cursor_before, scroll);
                document.history.break_run();
            }
            count = positions.len();
            if let Some(first) = positions.first() {
                pane.cursor = (*first).min(document.len_chars());
                pane.anchor = None;
            }
        }
        if count == 0 {
            self.notify("nothing replaced");
            return Ok(());
        }
        self.notify(format!("replaced {count} occurrence(s)"));
        self.ensure_cursor_visible();
        Ok(())
    }
}

/// Char offsets of every non-overlapping match.
fn match_positions(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(found) = haystack[offset..].find(needle) {
        let byte = offset + found;
        out.push(haystack[..byte].chars().count());
        offset = byte + needle.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::match_positions;

    #[test]
    fn finds_non_overlapping_matches() {
        assert_eq!(match_positions("aaaa", "aa"), vec![0, 2]);
        assert_eq!(match_positions("abcabc", "bc"), vec![1, 4]);
        assert!(match_positions("abc", "zz").is_empty());
        assert!(match_positions("abc", "").is_empty());
    }

    #[test]
    fn reports_char_offsets_not_bytes() {
        assert_eq!(match_positions("\u{4f60}\u{597d}!", "!"), vec![2]);
    }
}
