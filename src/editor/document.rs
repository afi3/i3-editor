//! A document owns the rope, its file identity, and its undo history.
//! It knows nothing about panes or screen geometry.

use std::cell::RefCell;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;

use crate::error::{Error, Result};
use crate::state::DocumentId;
use crate::syntax::{self, Language, LexState, Span};

use super::history::History;

/// Derived highlighting state: the lexer state at the start of each line whose
/// start has been computed. Cleared from the edited line onward, so only the
/// lines a frame needs are ever re-lexed after an edit.
#[derive(Debug, Default)]
pub(crate) struct SyntaxCache {
    language: Language,
    states: Vec<LexState>,
}

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
    /// Highlighting memo; derived from `rope`, never part of the file identity.
    pub(crate) syntax: RefCell<SyntaxCache>,
    /// Modification time and size seen at the last load or save, to detect
    /// writes made by other programs.
    pub(crate) disk_modified: Option<SystemTime>,
    pub(crate) disk_len: u64,
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
            syntax: RefCell::new(SyntaxCache::default()),
            disk_modified: None,
            disk_len: 0,
        }
    }

    pub fn open(id: DocumentId, path: PathBuf) -> Result<Self> {
        let bytes = std::fs::read(&path).map_err(|error| Error::io("read", &path, error))?;
        let text =
            String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 { path: path.clone() })?;
        let eol = Eol::detect(&text);
        let mut document = Self {
            id,
            path: Some(path),
            rope: Rope::from_str(&Eol::Lf.normalize(&text)),
            dirty: false,
            eol,
            history: History::default(),
            syntax: RefCell::new(SyntaxCache::default()),
            disk_modified: None,
            disk_len: 0,
        };
        document.record_disk_state();
        Ok(document)
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

    /// Highlight spans for `line`, using the cached lexer state from the lines
    /// above it. Filling the cache is derived work: rendering can call this
    /// through `&self` without changing the document.
    pub fn syntax_spans(&self, line: usize, text: &str) -> Vec<Span> {
        let language = Language::from_path(self.path.as_deref());
        let mut cache = self.syntax.borrow_mut();
        if cache.language != language {
            cache.language = language;
            cache.states.clear();
        }
        let lines = self.line_count().max(1);
        if cache.states.is_empty() {
            cache.states.push(LexState::Normal);
        } else if cache.states.len() > lines {
            cache.states.truncate(lines);
        }
        let target = line.min(lines - 1);
        while cache.states.len() <= target {
            let previous = cache.states.len() - 1;
            let previous_text = self.line_text(previous);
            let (_, next) = syntax::line_spans(language, &previous_text, cache.states[previous]);
            cache.states.push(next);
        }
        syntax::line_spans(language, text, cache.states[target]).0
    }

    /// The edited line keeps its start state; later lines are re-lexed lazily.
    fn invalidate_syntax(&mut self, line: usize) {
        self.syntax
            .get_mut()
            .states
            .truncate(line.saturating_add(1));
    }

    fn reset_syntax(&mut self) {
        self.syntax.get_mut().states.clear();
    }

    pub fn insert(
        &mut self,
        char_idx: usize,
        text: &str,
        cursor_before: usize,
        scroll_before: (usize, u16),
    ) {
        let char_idx = char_idx.min(self.len_chars());
        let line = self.line_of_char(char_idx);
        self.rope.insert(char_idx, text);
        self.history
            .record_insert(char_idx, text, cursor_before, scroll_before);
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
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
        let line = self.line_of_char(start);
        let text: String = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        self.history
            .record_remove(start, &text, cursor_before, scroll_before);
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
        text
    }

    /// Returns `(cursor, scroll)` to restore on the pane that issued the edit.
    pub fn undo(&mut self) -> Option<(usize, (usize, u16))> {
        let restore = self.history.undo(&mut self.rope)?;
        self.dirty = self.history.is_modified();
        self.reset_syntax();
        Some(restore)
    }

    pub fn redo(&mut self) -> Option<(usize, (usize, u16))> {
        let restore = self.history.redo(&mut self.rope)?;
        self.dirty = self.history.is_modified();
        self.reset_syntax();
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
        write_atomic(&path, text.as_bytes()).map_err(|error| Error::io("write", &path, error))?;
        self.path = Some(path);
        self.history.mark_saved();
        self.dirty = false;
        self.record_disk_state();
        Ok(())
    }

    /// True when another program changed or removed the file since the last
    /// load or save. Untitled buffers and unknown metadata never conflict.
    pub fn disk_changed(&self) -> bool {
        let (Some(path), Some(recorded)) = (&self.path, self.disk_modified) else {
            return false;
        };
        match std::fs::metadata(path) {
            Ok(metadata) => {
                metadata.modified().ok() != Some(recorded) || metadata.len() != self.disk_len
            }
            Err(_) => true,
        }
    }

    /// Replace the buffer with the file on disk, dropping undo history.
    pub fn reload(&mut self) -> Result<()> {
        let path = self.path.clone().ok_or(Error::NoPath)?;
        let bytes = std::fs::read(&path).map_err(|error| Error::io("read", &path, error))?;
        let text =
            String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 { path: path.clone() })?;
        self.eol = Eol::detect(&text);
        self.rope = Rope::from_str(&Eol::Lf.normalize(&text));
        self.history = History::default();
        self.dirty = false;
        self.reset_syntax();
        self.record_disk_state();
        Ok(())
    }

    pub(crate) fn record_disk_state(&mut self) {
        let metadata = self
            .path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok());
        match metadata {
            Some(metadata) => {
                self.disk_modified = metadata.modified().ok();
                self.disk_len = metadata.len();
            }
            None => {
                self.disk_modified = None;
                self.disk_len = 0;
            }
        }
    }
}

/// Replace `path` without ever truncating the original: write a sibling temp
/// file, flush it, then rename over the target. Symlinks are followed so the
/// link survives. Existing permissions are preserved.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string());
    let temporary = parent.join(format!(".{name}.i3-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        if let Ok(metadata) = std::fs::metadata(&target) {
            file.set_permissions(metadata.permissions())?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
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
    fn syntax_cache_follows_edits_and_undo() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/test.rs")));
        document.rope = Rope::from_str("/* a\nb */ let x = 1;\n");

        let text = document.line_text(1);
        let spans = document.syntax_spans(1, &text);
        assert!(
            spans.iter().any(|span| span.kind == TokenKind::Comment),
            "second line starts inside the comment"
        );

        document.remove(0..2, 0, (0, 0));
        for line in 0..2 {
            let text = document.line_text(line);
            let spans = document.syntax_spans(line, &text);
            assert!(
                !spans.iter().any(|span| span.kind == TokenKind::Comment),
                "line {line} after removing the opener"
            );
        }

        document.undo();
        let text = document.line_text(1);
        let spans = document.syntax_spans(1, &text);
        assert!(
            spans.iter().any(|span| span.kind == TokenKind::Comment),
            "undo restores the comment state"
        );
    }

    #[test]
    fn save_replaces_atomically_and_keeps_permissions() {
        let dir = std::env::temp_dir().join(format!("i3-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("script.sh");
        std::fs::write(&path, "old\n").expect("seed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("mode");
        }

        let mut document = doc("new\n");
        document.save_as(path.clone()).expect("save");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o640);
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_keeps_the_link() {
        let dir = std::env::temp_dir().join(format!("i3-symlink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let target = dir.join("real.txt");
        std::fs::write(&target, "old\n").expect("seed");
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let mut document = doc("new\n");
        document.save_as(link.clone()).expect("save");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "new\n");
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
