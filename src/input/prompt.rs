//! The prompt behind every dialog.

use std::path::PathBuf;

use crate::state::DocumentId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    CommandPalette,
    QuickOpen,
    Find,
    Replace,
    SaveAs,
    NewFile,
    NewDir,
    Rename,
    Delete,
    Discard,
    QuitDirty,
    GoToLine,
    Theme,
}

impl PromptKind {
    pub fn title(self) -> &'static str {
        match self {
            PromptKind::CommandPalette => "Command Palette",
            PromptKind::QuickOpen => "Quick Open",
            PromptKind::Find => "Find",
            PromptKind::Replace => "Replace",
            PromptKind::SaveAs => "Save As",
            PromptKind::NewFile => "New File",
            PromptKind::NewDir => "New Directory",
            PromptKind::Rename => "Rename",
            PromptKind::Delete => "Delete",
            PromptKind::Discard => "Unsaved changes: Enter discards, Esc cancels",
            PromptKind::QuitDirty => "Unsaved changes",
            PromptKind::GoToLine => "Go to Line",
            PromptKind::Theme => "Theme",
        }
    }

    /// Number of text fields. Replace has two: pattern and replacement.
    pub fn fields(self) -> usize {
        match self {
            PromptKind::Replace => 2,
            PromptKind::Delete | PromptKind::Discard | PromptKind::QuitDirty => 0,
            _ => 1,
        }
    }

    pub fn lists_candidates(self) -> bool {
        matches!(
            self,
            PromptKind::CommandPalette
                | PromptKind::QuickOpen
                | PromptKind::Theme
                | PromptKind::QuitDirty
        )
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
        }
    }

    pub fn with_field(mut self, value: impl Into<String>) -> Self {
        if let Some(field) = self.fields.get_mut(self.active) {
            *field = value.into();
            self.cursor = field.chars().count();
        }
        self
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
        self.refilter();
    }

    pub fn refilter(&mut self) {
        let needle = self.input().to_ascii_lowercase();
        self.filtered = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                needle.is_empty() || candidate.to_ascii_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();
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
    }

    pub fn next_field(&mut self) {
        if self.fields.len() <= 1 {
            return;
        }
        self.active = (self.active + 1) % self.fields.len();
        self.cursor = self.input().chars().count();
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
    fn replace_has_two_fields() {
        let mut prompt = PromptState::new(PromptKind::Replace);
        prompt.insert_char('a');
        prompt.next_field();
        prompt.insert_char('b');
        assert_eq!(prompt.fields, vec!["a".to_string(), "b".to_string()]);
    }
}
