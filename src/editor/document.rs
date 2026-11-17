//! A document owns the rope, its file identity, and its undo history.
//! It knows nothing about panes or screen geometry.

use std::ops::Range;
use std::path::PathBuf;

use ropey::Rope;

use crate::error::{Error, Result};
use crate::state::DocumentId;

use super::history::History;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }

    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Eol::Crlf
        } else {
            Eol::Lf
        }
    }

    /// Convert any line endings to `self`.
    pub fn normalize(self, text: &str) -> String {
        let lf = text.replace("\r\n", "\n").replace('\r', "\n");
        match self {
            Eol::Lf => lf,
            Eol::Crlf => lf.replace('\n', "\r\n"),
        }
    }
}

#[derive(Debug)]
pub struct Document {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    pub rope: Rope,
    pub dirty: bool,
    pub eol: Eol,
    pub history: History,
}

impl Document {
    pub fn empty(id: DocumentId, path: Option<PathBuf>) -> Self {
        Self {
            id,
            path,
            rope: Rope::new(),
            dirty: false,
            eol: Eol::Lf,
            history: History::default(),
        }
    }

    pub fn open(id: DocumentId, path: PathBuf) -> Result<Self> {
        let bytes = std::fs::read(&path).map_err(|error| Error::io("read", &path, error))?;
        let text =
            String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 { path: path.clone() })?;
        let eol = Eol::detect(&text);
        Ok(Self {
            id,
            path: Some(path),
            rope: Rope::from_str(&Eol::Lf.normalize(&text)),
            dirty: false,
            eol,
            history: History::default(),
        })
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[No Name]".to_string())
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_of_char(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx.min(self.len_chars()))
    }

    pub fn char_of_line(&self, line: usize) -> usize {
        self.rope
            .line_to_char(line.min(self.line_count().saturating_sub(1)))
    }

    /// Line text without its terminator.
    pub fn line_text(&self, line: usize) -> String {
        let mut text = self.rope.line(line).to_string();
        while text.ends_with('\n') || text.ends_with('\r') {
            text.pop();
        }
        text
    }

    pub fn insert(
        &mut self,
        char_idx: usize,
        text: &str,
        cursor_before: usize,
        scroll_before: (usize, u16),
    ) {
        let char_idx = char_idx.min(self.len_chars());
        self.rope.insert(char_idx, text);
        self.history
            .record_insert(char_idx, text, cursor_before, scroll_before);
        self.dirty = true;
    }

    pub fn remove(
        &mut self,
        range: Range<usize>,
        cursor_before: usize,
        scroll_before: (usize, u16),
    ) -> String {
        let start = range.start.min(self.len_chars());
        let end = range.end.min(self.len_chars());
        if start >= end {
            return String::new();
        }
        let text: String = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        self.history
            .record_remove(start, &text, cursor_before, scroll_before);
        self.dirty = true;
        text
    }

    /// Returns `(cursor, scroll)` to restore on the pane that issued the edit.
    pub fn undo(&mut self) -> Option<(usize, (usize, u16))> {
        let restore = self.history.undo(&mut self.rope)?;
        self.dirty = true;
        Some(restore)
    }

    pub fn redo(&mut self) -> Option<(usize, (usize, u16))> {
        let restore = self.history.redo(&mut self.rope)?;
        self.dirty = true;
        Some(restore)
    }

    pub fn save(&mut self) -> Result<()> {
        let path = self.path.clone().ok_or(Error::NoPath)?;
        self.write_to(path)
    }

    pub fn save_as(&mut self, path: PathBuf) -> Result<()> {
        self.write_to(path)
    }

    fn write_to(&mut self, path: PathBuf) -> Result<()> {
        let text = self.eol.normalize(&self.rope.to_string());
        std::fs::write(&path, text).map_err(|error| Error::io("write", &path, error))?;
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        let mut document = Document::empty(DocumentId(1), None);
        document.rope = Rope::from_str(text);
        document
    }

    #[test]
    fn insert_and_remove_track_history() {
        let mut document = doc("hello");
        document.insert(5, " world", 5, (0, 0));
        assert_eq!(document.rope.to_string(), "hello world");
        assert!(document.dirty);
        document.remove(0..6, 6, (0, 0));
        assert_eq!(document.rope.to_string(), "world");
        document.undo();
        assert_eq!(document.rope.to_string(), "hello world");
        document.undo();
        assert_eq!(document.rope.to_string(), "hello");
        document.redo();
        assert_eq!(document.rope.to_string(), "hello world");
    }

    #[test]
    fn undo_restores_cursor_before_the_batch() {
        let mut document = doc("abc");
        document.insert(3, "d", 3, (0, 0));
        let (cursor, scroll) = document.undo().expect("undo");
        assert_eq!(cursor, 3);
        assert_eq!(scroll, (0, 0));
        let (cursor, _) = document.redo().expect("redo");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn insertion_runs_coalesce() {
        let mut document = doc("");
        for (idx, ch) in ["a", "b", "c"].iter().enumerate() {
            document.insert(idx, ch, idx, (0, 0));
        }
        assert_eq!(document.history.undo_depth(), 1);
        document.undo();
        assert_eq!(document.rope.to_string(), "");
    }

    #[test]
    fn newline_breaks_the_coalescing_run() {
        let mut document = doc("");
        document.insert(0, "a", 0, (0, 0));
        document.insert(1, "\n", 1, (0, 0));
        document.insert(2, "b", 2, (0, 0));
        assert_eq!(document.history.undo_depth(), 3);
    }

    #[test]
    fn save_as_preserves_line_endings() {
        let dir = std::env::temp_dir().join(format!("i3-editor-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("crlf.txt");
        let mut document = doc("one\ntwo");
        document.eol = Eol::Crlf;
        document.save_as(path.clone()).expect("save");
        let written = std::fs::read_to_string(&path).expect("read");
        assert_eq!(written, "one\r\ntwo");
        assert!(!document.dirty);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn line_helpers_agree_with_the_rope() {
        let document = doc("abc\ndef\nghi");
        assert_eq!(document.line_count(), 3);
        assert_eq!(document.line_text(1), "def");
        assert_eq!(document.line_of_char(5), 1);
        assert_eq!(document.char_of_line(2), 8);
    }
}
