//! The prompt behind every dialog.

use std::path::PathBuf;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};

use crate::state::DocumentId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    CommandPalette,
    Messages,
    OpenPath,
    QuickOpen,
    Find,
    FindInFiles,
    Replace,
    SaveAs,
    Overwrite,
    ExternalChange,
    NewFile,
    NewDir,
    Rename,
    Delete,
    Discard,
    QuitDirty,
    GoToLine,
    Theme,
}

/// Grouping used by the prompt setup and submit code, so neither needs one
/// giant match over every kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptGroup {
    Query,
    File,
    Session,
}

impl PromptKind {
    pub fn group(self) -> PromptGroup {
        if self.is_query() {
            PromptGroup::Query
        } else if self.is_file() {
            PromptGroup::File
        } else {
            PromptGroup::Session
        }
    }

    fn is_query(self) -> bool {
        matches!(
            self,
            PromptKind::CommandPalette
                | PromptKind::Messages
                | PromptKind::Find
                | PromptKind::FindInFiles
                | PromptKind::Replace
                | PromptKind::GoToLine
        )
    }

    fn is_file(self) -> bool {
        matches!(
            self,
            PromptKind::OpenPath
                | PromptKind::QuickOpen
                | PromptKind::SaveAs
                | PromptKind::Overwrite
                | PromptKind::ExternalChange
                | PromptKind::NewFile
                | PromptKind::NewDir
                | PromptKind::Rename
                | PromptKind::Delete
        )
    }

    pub fn title(self) -> &'static str {
        match self.group() {
            PromptGroup::Query => self.query_title(),
            PromptGroup::File => self.file_title(),
            PromptGroup::Session => self.session_title(),
        }
    }

    fn query_title(self) -> &'static str {
        match self {
            PromptKind::CommandPalette => "Command Palette",
            PromptKind::Messages => "Messages",
            PromptKind::Find => "Find",
            PromptKind::FindInFiles => "Find in Files",
            PromptKind::Replace => "Replace",
            PromptKind::GoToLine => "Go to Line",
            _ => "",
        }
    }

    fn file_title(self) -> &'static str {
        match self {
            PromptKind::OpenPath => "Open File",
            PromptKind::QuickOpen => "Quick Open",
            PromptKind::SaveAs => "Save As",
            PromptKind::Overwrite => "Overwrite file",
            PromptKind::ExternalChange => "File changed on disk",
            PromptKind::NewFile => "New File",
            PromptKind::NewDir => "New Directory",
            PromptKind::Rename => "Rename",
            PromptKind::Delete => "Delete",
            _ => "",
        }
    }

    fn session_title(self) -> &'static str {
        match self {
            PromptKind::Discard => "Unsaved changes: Enter discards, Esc cancels",
            PromptKind::QuitDirty => "Unsaved changes",
            PromptKind::Theme => "Theme",
            _ => "",
        }
    }

    /// Number of text fields. Replace has two: pattern and replacement.
    pub fn fields(self) -> usize {
        match self {
            PromptKind::Replace => 2,
            PromptKind::Delete
            | PromptKind::Messages
            | PromptKind::Discard
            | PromptKind::QuitDirty
            | PromptKind::Overwrite
            | PromptKind::ExternalChange => 0,
            _ => 1,
        }
    }

    pub fn lists_candidates(self) -> bool {
        matches!(
            self,
            PromptKind::CommandPalette
                | PromptKind::Messages
                | PromptKind::QuickOpen
                | PromptKind::FindInFiles
                | PromptKind::Theme
                | PromptKind::QuitDirty
                | PromptKind::ExternalChange
        )
    }
}

/// Candidate wrapper so a scored list can still report the original index.
struct Candidate<'a> {
    index: usize,
    text: &'a str,
}

impl AsRef<str> for Candidate<'_> {
    fn as_ref(&self) -> &str {
        self.text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptState {
    pub kind: PromptKind,
    pub fields: Vec<String>,
    pub active: usize,
    pub cursor: usize,
    pub candidates: Vec<String>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub target: Option<PathBuf>,
    pub document: Option<DocumentId>,
    /// Set when the user moves the selection with arrows, so typing keeps that
    /// candidate instead of jumping back to the best match.
    keep_selection: bool,
}

impl PromptState {
    pub fn new(kind: PromptKind) -> Self {
        Self {
            kind,
            fields: vec![String::new(); kind.fields()],
            active: 0,
            cursor: 0,
            candidates: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            target: None,
            document: None,
            keep_selection: false,
        }
    }

    /// Replace the active field and place the cursor at its end.
    pub fn set_input(&mut self, value: impl Into<String>) {
        if let Some(field) = self.fields.get_mut(self.active) {
            *field = value.into();
            self.cursor = field.chars().count();
        }
    }

    pub fn input(&self) -> &str {
        self.fields
            .get(self.active)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn visible(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .filter_map(|index| self.candidates.get(*index).map(String::as_str))
            .collect()
    }

    pub fn set_candidates(&mut self, candidates: Vec<String>) {
        self.candidates = candidates;
        self.selected = 0;
        self.keep_selection = false;
        self.refilter();
    }

    pub fn refilter(&mut self) {
        // Project-search candidates are matches for the query, not fuzzy
        // candidates; filtering them again would hide results.
        if self.kind == PromptKind::FindInFiles {
            self.filtered = (0..self.candidates.len()).collect();
            self.selected = self.selected.min(self.candidates.len().saturating_sub(1));
            return;
        }
        let selected = self.selected_candidate().map(str::to_string);
        let needle = self.input();
        if needle.is_empty() {
            self.filtered = (0..self.candidates.len()).collect();
        } else {
            let pattern = Pattern::parse(needle, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let scored = pattern.match_list(
                self.candidates
                    .iter()
                    .enumerate()
                    .map(|(index, text)| Candidate { index, text }),
                &mut matcher,
            );
            self.filtered = scored
                .into_iter()
                .map(|(candidate, _score)| candidate.index)
                .collect();
        }
        // Arrow-key selections follow their candidate through the new ranking;
        // otherwise the best match wins, so Enter opens what the ranking shows.
        let kept = self
            .keep_selection
            .then_some(selected)
            .flatten()
            .and_then(|name| {
                self.filtered
                    .iter()
                    .position(|index| self.candidates[*index] == name)
            });
        self.selected = kept.unwrap_or(0);
        self.keep_selection = kept.is_some();
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    /// Preselect `needle` in the filtered candidate list.
    pub fn select_candidate(&mut self, needle: &str) {
        self.refilter();
        if let Some(index) = self
            .filtered
            .iter()
            .position(|index| self.candidates[*index].eq_ignore_ascii_case(needle))
        {
            self.selected = index;
            self.keep_selection = true;
        }
    }

    pub fn selected_candidate(&self) -> Option<&str> {
        let index = self.filtered.get(self.selected)?;
        self.candidates.get(*index).map(String::as_str)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let last = self.filtered.len() - 1;
        let next = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        self.selected = next;
        self.keep_selection = true;
    }

    pub fn next_field(&mut self) {
        if self.fields.len() <= 1 {
            return;
        }
        self.active = (self.active + 1) % self.fields.len();
        self.cursor = self.input().chars().count();
    }

    /// Insert pasted text at the cursor; prompts stay single-line so callers
    /// decide whether newlines are meaningful for the field.
    pub fn insert_text(&mut self, text: &str) {
        let byte = byte_index(self.input(), self.cursor);
        if let Some(field) = self.fields.get_mut(self.active) {
            field.insert_str(byte, text);
        }
        self.cursor += text.chars().count();
        self.refilter();
    }

    pub fn insert_char(&mut self, ch: char) {
        let byte = byte_index(self.input(), self.cursor);
        if let Some(field) = self.fields.get_mut(self.active) {
            field.insert(byte, ch);
        }
        self.cursor += 1;
        self.refilter();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let byte = byte_index(self.input(), self.cursor - 1);
        let next = byte_index(self.input(), self.cursor);
        if let Some(field) = self.fields.get_mut(self.active) {
            field.replace_range(byte..next, "");
        }
        self.cursor -= 1;
        self.refilter();
    }

    pub fn delete(&mut self) {
        let byte = byte_index(self.input(), self.cursor);
        let next = byte_index(self.input(), self.cursor + 1);
        if byte == next {
            return;
        }
        if let Some(field) = self.fields.get_mut(self.active) {
            field.replace_range(byte..next, "");
        }
        self.refilter();
    }

    pub fn cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.input().chars().count());
    }
}

fn byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_respects_multibyte_input() {
        let mut prompt = PromptState::new(PromptKind::QuickOpen);
        prompt.insert_char('a');
        prompt.insert_char('\u{4f60}');
        prompt.cursor_left();
        prompt.insert_char('b');
        assert_eq!(prompt.input(), "ab\u{4f60}");
        prompt.backspace();
        assert_eq!(prompt.input(), "a\u{4f60}");
    }

    #[test]
    fn candidates_filter_case_insensitively() {
        let mut prompt = PromptState::new(PromptKind::CommandPalette);
        prompt.set_candidates(vec![
            "Save".to_string(),
            "Save As".to_string(),
            "Quit".to_string(),
        ]);
        prompt.insert_char('s');
        prompt.insert_char('a');
        assert_eq!(prompt.visible(), vec!["Save", "Save As"]);
        prompt.move_selection(1);
        assert_eq!(prompt.selected_candidate(), Some("Save As"));
    }

    #[test]
    fn fuzzy_matching_accepts_scattered_initials() {
        let mut prompt = PromptState::new(PromptKind::QuickOpen);
        prompt.set_candidates(vec![
            "docs/domain_main.md".to_string(),
            "src/app.rs".to_string(),
            "src/main.rs".to_string(),
        ]);
        prompt.insert_char('s');
        prompt.insert_char('m');
        assert_eq!(prompt.selected_candidate(), Some("src/main.rs"));
        assert_eq!(prompt.visible(), vec!["src/main.rs", "docs/domain_main.md"]);
    }

    #[test]
    fn fuzzy_matching_ranks_boundaries_first() {
        let mut prompt = PromptState::new(PromptKind::CommandPalette);
        prompt.set_candidates(vec![
            "split_top_bottom".to_string(),
            "select_theme".to_string(),
            "toggle_terminal".to_string(),
        ]);
        prompt.insert_char('t');
        prompt.insert_char('t');
        assert_eq!(prompt.selected_candidate(), Some("toggle_terminal"));
    }

    #[test]
    fn fuzzy_matching_is_smart_about_case() {
        let mut prompt = PromptState::new(PromptKind::QuickOpen);
        prompt.set_candidates(vec!["src/main.rs".to_string(), "SRC/MAIN.RS".to_string()]);
        prompt.insert_char('S');
        prompt.insert_char('M');
        assert_eq!(prompt.selected_candidate(), Some("SRC/MAIN.RS"));
    }

    #[test]
    fn empty_query_keeps_order_and_selection() {
        let mut prompt = PromptState::new(PromptKind::Theme);
        prompt.set_candidates(vec!["b".to_string(), "a".to_string(), "c".to_string()]);
        prompt.move_selection(2);
        prompt.refilter();
        assert_eq!(prompt.visible(), vec!["b", "a", "c"]);
        assert_eq!(prompt.selected_candidate(), Some("c"));
    }

    #[test]
    fn selection_survives_a_narrowing_filter() {
        let mut prompt = PromptState::new(PromptKind::QuickOpen);
        prompt.set_candidates(vec![
            "src/main.rs".to_string(),
            "src/mode.rs".to_string(),
            "README.md".to_string(),
        ]);
        prompt.move_selection(1);
        prompt.insert_char('m');
        assert_eq!(prompt.selected_candidate(), Some("src/mode.rs"));
    }

    #[test]
    fn selection_falls_back_to_the_best_match() {
        let mut prompt = PromptState::new(PromptKind::QuickOpen);
        prompt.set_candidates(vec![
            "src/main.rs".to_string(),
            "src/mode.rs".to_string(),
            "README.md".to_string(),
        ]);
        prompt.move_selection(2);
        for ch in "src".chars() {
            prompt.insert_char(ch);
        }
        assert_eq!(prompt.selected_candidate(), Some("src/main.rs"));
    }

    #[test]
    fn fuzzy_matching_reports_no_matches() {
        let mut prompt = PromptState::new(PromptKind::QuickOpen);
        prompt.set_candidates(vec!["src/main.rs".to_string()]);
        for ch in "zzz".chars() {
            prompt.insert_char(ch);
        }
        assert!(prompt.visible().is_empty());
        assert_eq!(prompt.selected_candidate(), None);
    }

    #[test]
    fn project_search_results_are_not_filtered_again() {
        let mut prompt = PromptState::new(PromptKind::FindInFiles);
        prompt.insert_char('z');
        prompt.set_candidates(vec!["a.txt:1:6  first needle".to_string()]);
        assert_eq!(prompt.visible(), vec!["a.txt:1:6  first needle"]);
        prompt.move_selection(1);
        assert_eq!(prompt.selected_candidate(), Some("a.txt:1:6  first needle"));
    }

    #[test]
    fn every_prompt_has_a_title() {
        for kind in [
            PromptKind::CommandPalette,
            PromptKind::Messages,
            PromptKind::OpenPath,
            PromptKind::QuickOpen,
            PromptKind::Find,
            PromptKind::FindInFiles,
            PromptKind::Replace,
            PromptKind::SaveAs,
            PromptKind::Overwrite,
            PromptKind::ExternalChange,
            PromptKind::NewFile,
            PromptKind::NewDir,
            PromptKind::Rename,
            PromptKind::Delete,
            PromptKind::Discard,
            PromptKind::QuitDirty,
            PromptKind::GoToLine,
            PromptKind::Theme,
        ] {
            assert!(!kind.title().is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn replace_has_two_fields() {
        let mut prompt = PromptState::new(PromptKind::Replace);
        prompt.insert_char('a');
        prompt.next_field();
        prompt.insert_char('b');
        assert_eq!(prompt.fields, vec!["a".to_string(), "b".to_string()]);
    }
}
