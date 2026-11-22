//! A document owns the rope, its file identity, and its undo history.
//! It knows nothing about panes or screen geometry.

use std::cell::RefCell;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;
use tree_sitter::{InputEdit, Point};

use crate::error::{Error, Result};
use crate::state::DocumentId;
use crate::syntax::tree::Highlighter;
use crate::syntax::{self, Language, LexState, Span};

use super::history::History;

/// Derived highlighting state: the lexer state at the start of each line whose
/// start has been computed, plus the tree-sitter parse tree for grammars that
/// have one. The lexer memo is cleared from the edited line onward; the tree is
/// updated with `InputEdit`s and reparsed lazily.
#[derive(Debug, Default)]
pub(crate) struct SyntaxCache {
    language: Language,
    states: Vec<LexState>,
    /// Parse tree for grammars supported by `syntax::tree`.
    tree: Highlighter,
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
    #[cfg(test)]
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

    /// Spans for a contiguous run of lines. Uses tree-sitter when the language
    /// has a grammar and falls back to the lexer otherwise.
    pub fn syntax_spans_range(&self, first: usize, count: usize) -> Vec<Vec<Span>> {
        let line_count = self.line_count();
        if count == 0 {
            return Vec::new();
        }
        let first = first.min(line_count.saturating_sub(1));
        let count = count.min(line_count - first);
        let language = Language::from_path(self.path.as_deref());
        if crate::syntax::tree::supports(language) {
            let mut cache = self.syntax.borrow_mut();
            if cache.language != language {
                cache.language = language;
                cache.states.clear();
                cache.tree.invalidate();
            }
            if let Some(spans) = cache.tree.spans(&self.rope, language, first, count) {
                return spans;
            }
        }
        (first..first + count)
            .map(|line| {
                let text = self.line_text(line);
                self.syntax_spans(line, &text)
            })
            .collect()
    }

    fn reset_syntax(&mut self) {
        let cache = self.syntax.get_mut();
        cache.states.clear();
        cache.tree.invalidate();
    }

    /// Describe an edit to the parse tree. Positions use the rope before the
    /// edit; `inserted` is the text that will replace `start..end`.
    fn input_edit(&self, start: usize, end: usize, inserted: &str) -> InputEdit {
        let start_byte = self.rope.char_to_byte(start);
        let start_row = self.rope.char_to_line(start);
        let start_point = Point {
            row: start_row,
            column: start_byte - self.rope.line_to_byte(start_row),
        };
        let old_end_byte = self.rope.char_to_byte(end);
        let old_end_row = self.rope.char_to_line(end);
        let old_end_point = Point {
            row: old_end_row,
            column: old_end_byte - self.rope.line_to_byte(old_end_row),
        };
        let rows = inserted.matches('\n').count();
        let new_end_point = if rows == 0 {
            Point {
                row: start_row,
                column: start_point.column + inserted.len(),
            }
        } else {
            Point {
                row: start_row + rows,
                column: inserted.rsplit('\n').next().unwrap_or("").len(),
            }
        };
        InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte + inserted.len(),
            start_position: start_point,
            old_end_position: old_end_point,
            new_end_position: new_end_point,
        }
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
        let edit = self.input_edit(char_idx, char_idx, text);
        self.rope.insert(char_idx, text);
        self.history
            .record_insert(char_idx, text, cursor_before, scroll_before);
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
        self.syntax.get_mut().tree.apply_edit(edit);
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
        let edit = self.input_edit(start, end, "");
        self.rope.remove(start..end);
        self.history
            .record_remove(start, &text, cursor_before, scroll_before);
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
        self.syntax.get_mut().tree.apply_edit(edit);
        text
    }

    /// Replace a range as one undo step. Used by structural edits that rebuild
    /// a line block instead of inserting character by character.
    pub fn replace(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        cursor_before: usize,
        cursor_after: usize,
        scroll_before: (usize, u16),
    ) {
        let start = range.start.min(self.len_chars());
        let end = range.end.min(self.len_chars());
        if start >= end {
            self.insert(start, text, cursor_before, scroll_before);
            return;
        }
        let line = self.line_of_char(start);
        let removed: String = self.rope.slice(start..end).to_string();
        let edit = self.input_edit(start, end, text);
        self.rope.remove(start..end);
        self.rope.insert(start, text);
        self.history.record_replace(
            start,
            &removed,
            text,
            cursor_before,
            cursor_after,
            scroll_before,
        );
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
        self.syntax.get_mut().tree.apply_edit(edit);
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
    fn structural_spans_cover_multiline_rust_raw_strings() {
        use crate::syntax::{Span, TokenKind};

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/raw.rs")));
        document.rope = Rope::from_str("let s = r#\"alpha\nbeta\"#;\n");
        let spans = document.syntax_spans_range(0, 2);
        assert_eq!(
            spans[1].first(),
            Some(&Span {
                start: 0,
                end: 6,
                kind: TokenKind::String,
            }),
            "the raw string survives the line break"
        );
    }

    #[test]
    fn structural_spans_cover_shell_heredocs() {
        use crate::syntax::{Span, TokenKind};

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/run.sh")));
        document.rope = Rope::from_str("cat <<EOF > out\nhello\nEOF\necho done\n");
        let spans = document.syntax_spans_range(1, 1);
        assert_eq!(
            spans[0],
            [Span {
                start: 0,
                end: 5,
                kind: TokenKind::String,
            }]
        );
    }

    #[test]
    fn spans_highlight_constructs_starting_above_the_viewport() {
        use crate::syntax::{Span, TokenKind};

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/scroll.rs")));
        document.rope = Rope::from_str("let s = r#\"\nalpha\nbeta\ngamma\"#;\nlet n = 1;\n");
        let spans = document.syntax_spans_range(2, 2);
        assert_eq!(
            spans[0],
            [Span {
                start: 0,
                end: 4,
                kind: TokenKind::String,
            }]
        );
        assert_eq!(
            spans[1],
            [
                Span {
                    start: 0,
                    end: 7,
                    kind: TokenKind::String,
                },
                Span {
                    start: 7,
                    end: 8,
                    kind: TokenKind::Punctuation,
                },
            ]
        );
    }

    #[test]
    fn structural_spans_cover_json_and_toml() {
        use crate::syntax::TokenKind;

        let mut json = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/data.json")));
        json.rope = Rope::from_str("{\"n\": 42, \"ok\": true}\n");
        let spans = json.syntax_spans_range(0, 1);
        assert!(
            spans[0].iter().any(|span| span.kind == TokenKind::Number),
            "{:?}",
            spans[0]
        );
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Constant));
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::String));

        let mut toml = Document::empty(DocumentId(2), Some(PathBuf::from("/tmp/data.toml")));
        toml.rope = Rope::from_str("enabled = true\ncount = 3\n");
        let spans = toml.syntax_spans_range(0, 2);
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Constant));
        assert!(spans[1].iter().any(|span| span.kind == TokenKind::Number));
    }

    #[test]
    fn structural_spans_cover_markdown_and_fenced_code() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/notes.md")));
        document.rope = Rope::from_str(
            "# Title\n\nText with `code` and [link](https://x.test).\n\n```rust\nfn main() {\n    // note\n}\n```\n",
        );
        let spans = document.syntax_spans_range(0, 9);

        assert!(
            spans[0].iter().any(|span| span.kind == TokenKind::TypeName),
            "heading: {:?}",
            spans[0]
        );
        assert!(
            spans[2].iter().any(|span| span.kind == TokenKind::String),
            "inline code: {:?}",
            spans[2]
        );
        assert!(
            spans[5].iter().any(|span| span.kind == TokenKind::Keyword),
            "fenced rust keyword: {:?}",
            spans[5]
        );
        assert!(
            spans[6].iter().any(|span| span.kind == TokenKind::Comment),
            "fenced rust comment: {:?}",
            spans[6]
        );
        assert!(
            !spans[6].iter().any(|span| span.kind == TokenKind::String),
            "fence content is not a blanket literal: {:?}",
            spans[6]
        );
    }

    #[test]
    fn structural_spans_cover_markdown_html() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/html.md")));
        document.rope = Rope::from_str(
            "<div class=\"note\">\nraw\n</div>\n\nText <span title=\"x\">b</span>.\n",
        );
        let spans = document.syntax_spans_range(0, 5);

        assert!(
            spans[0].iter().any(|span| span.kind == TokenKind::Keyword),
            "html block tag: {:?}",
            spans[0]
        );
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::String));
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Function));
        assert!(spans[1].is_empty(), "raw html text: {:?}", spans[1]);
        assert!(
            spans[2].iter().any(|span| span.kind == TokenKind::Keyword),
            "closing tag: {:?}",
            spans[2]
        );
        assert!(
            spans[4].iter().any(|span| span.kind == TokenKind::Keyword),
            "inline html tag: {:?}",
            spans[4]
        );
        assert!(spans[4].iter().any(|span| span.kind == TokenKind::String));
        assert!(spans[4].iter().any(|span| span.kind == TokenKind::Function));
    }

    #[test]
    fn structural_spans_cover_yaml_front_matter() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/front.md")));
        document.rope = Rope::from_str("---\ntitle: Hello\ndraft: true\ncount: 3\n---\n# Body\n");
        let spans = document.syntax_spans_range(0, 6);

        assert!(
            spans[1].iter().any(|span| span.kind == TokenKind::String),
            "yaml string: {:?}",
            spans[1]
        );
        assert!(
            spans[2].iter().any(|span| span.kind == TokenKind::Constant),
            "yaml boolean: {:?}",
            spans[2]
        );
        assert!(
            spans[3].iter().any(|span| span.kind == TokenKind::Number),
            "yaml number: {:?}",
            spans[3]
        );
        assert!(
            spans[5].iter().any(|span| span.kind == TokenKind::TypeName),
            "markdown heading after front matter: {:?}",
            spans[5]
        );
    }

    #[test]
    fn structural_spans_cover_lua() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/init.lua")));
        document.rope = Rope::from_str("local n = 42\n-- note\nfunction f() return true end\n");
        let spans = document.syntax_spans_range(0, 3);

        assert!(
            spans[0].iter().any(|span| span.kind == TokenKind::Keyword),
            "lua keyword: {:?}",
            spans[0]
        );
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Number));
        assert!(
            spans[1].iter().any(|span| span.kind == TokenKind::Comment),
            "lua comment: {:?}",
            spans[1]
        );
        assert!(
            spans[2].iter().any(|span| span.kind == TokenKind::Keyword),
            "lua function keyword: {:?}",
            spans[2]
        );
    }

    #[test]
    fn structural_spans_cover_toml_front_matter() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/front.md")));
        document.rope =
            Rope::from_str("+++\ntitle = \"Hello\"\ndraft = true\ncount = 3\n+++\n# Body\n");
        let spans = document.syntax_spans_range(0, 6);

        assert!(
            spans[1].iter().any(|span| span.kind == TokenKind::String),
            "toml string: {:?}",
            spans[1]
        );
        assert!(
            spans[2].iter().any(|span| span.kind == TokenKind::Constant),
            "toml boolean: {:?}",
            spans[2]
        );
        assert!(
            spans[3].iter().any(|span| span.kind == TokenKind::Number),
            "toml number: {:?}",
            spans[3]
        );
        assert!(
            spans[5].iter().any(|span| span.kind == TokenKind::TypeName),
            "markdown heading after front matter: {:?}",
            spans[5]
        );
    }

    #[test]
    fn markdown_injections_follow_edits() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/edit.md")));
        document.rope = Rope::from_str("```rust\nlet x = 1;\n```\n");
        let spans = document.syntax_spans_range(1, 1);
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Keyword));

        document.insert(8, "// ", 8, (0, 0));
        let spans = document.syntax_spans_range(1, 1);
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Comment));
        assert!(!spans[0].iter().any(|span| span.kind == TokenKind::Keyword));
    }

    #[test]
    fn edits_update_the_parse_tree() {
        use crate::syntax::TokenKind;

        let mut document = Document::empty(DocumentId(1), Some(PathBuf::from("/tmp/edit.rs")));
        document.rope = Rope::from_str("let a = 1;\nlet b = 2;\n");
        let spans = document.syntax_spans_range(0, 2);
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Keyword));

        document.insert(0, "/*", 0, (0, 0));
        let end = document.len_chars();
        document.insert(end, "*/\n", end, (0, 0));

        let spans = document.syntax_spans_range(0, 3);
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Comment));
        assert!(spans[1].iter().any(|span| span.kind == TokenKind::Comment));

        document.undo();
        document.undo();
        let spans = document.syntax_spans_range(0, 2);
        assert!(!spans[0].iter().any(|span| span.kind == TokenKind::Comment));
        assert!(spans[0].iter().any(|span| span.kind == TokenKind::Keyword));
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
