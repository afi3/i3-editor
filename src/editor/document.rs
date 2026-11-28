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

use super::history::{Edit, History};

/// Files above this size are refused instead of being copied into multiple
/// in-memory representations.
pub const MAX_FILE_BYTES: u64 = 16 << 20;

/// Hard ceiling for the read-only large-file mode.
pub const MAX_READ_ONLY_BYTES: u64 = 256 << 20;

fn read_text(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path).map_err(|error| Error::io("read", path, error))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Error::FileTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: MAX_FILE_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|error| Error::io("read", path, error))?;
    String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 {
        path: path.to_path_buf(),
    })
}

/// Detect CRLF without materialising the whole document as a string.
fn detect_rope_eol(rope: &Rope) -> Eol {
    let mut chars = rope.chars();
    while let Some(ch) = chars.next() {
        if ch == '\r' && matches!(chars.next(), Some('\n')) {
            return Eol::Crlf;
        }
    }
    Eol::Lf
}

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

    /// Terminator inserted for a new line. Existing bytes are never rewritten.
    pub fn ending(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }
}

#[derive(Debug)]
pub struct Document {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    rope: Rope,
    pub dirty: bool,
    /// Large-file mode: the buffer is viewable but never mutated or saved.
    pub read_only: bool,
    pub eol: Eol,
    /// Per-document indentation from `.editorconfig` and language defaults.
    pub indent: crate::editorconfig::IndentSettings,
    pub history: History,
    /// Closed folds as inclusive document line ranges. Ordinary edits
    /// re-anchor them; undo/redo and full reloads clear them.
    pub folded: Vec<Range<usize>>,
    /// Highlighting memo; derived from `rope`, never part of the file identity.
    pub(crate) syntax: RefCell<SyntaxCache>,
    /// Generation counter for the rope; bumped by every text change so
    /// observers such as the language server can skip unchanged buffers.
    pub(crate) revision: u64,
    /// Modification time and size seen at the last load or save, to detect
    /// writes made by other programs.
    pub(crate) disk_modified: Option<SystemTime>,
    pub(crate) disk_len: u64,
}

impl Document {
    /// Read-only view of the buffer. Every mutation goes through a method that
    /// bumps `revision`, so no observer can miss an edit.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Build a buffer from text without touching the disk. Test fixtures only:
    /// production text arrives through `open`, `reload`, or edits.
    #[cfg(test)]
    pub fn from_text(id: DocumentId, text: &str, path: Option<PathBuf>) -> Self {
        let mut document = Self::empty(id, path);
        document.rope = Rope::from_str(text);
        document.eol = Eol::detect(text);
        document.touch();
        document
    }

    pub fn empty(id: DocumentId, path: Option<PathBuf>) -> Self {
        Self {
            id,
            path,
            rope: Rope::new(),
            dirty: false,
            read_only: false,
            eol: Eol::Lf,
            indent: crate::editorconfig::IndentSettings::default(),
            history: History::default(),
            folded: Vec::new(),
            syntax: RefCell::new(SyntaxCache::default()),
            revision: 0,
            disk_modified: None,
            disk_len: 0,
        }
    }

    pub fn open(id: DocumentId, path: PathBuf) -> Result<Self> {
        let text = read_text(&path)?;
        let eol = Eol::detect(&text);
        let mut document = Self {
            id,
            path: Some(path),
            rope: Rope::from_str(&text),
            dirty: false,
            read_only: false,
            eol,
            indent: crate::editorconfig::IndentSettings::default(),
            history: History::default(),
            folded: Vec::new(),
            syntax: RefCell::new(SyntaxCache::default()),
            revision: 0,
            disk_modified: None,
            disk_len: 0,
        };
        document.record_disk_state();
        Ok(document)
    }

    /// Open a file above the normal limit as a viewable, never-editable buffer.
    /// The rope is built by streaming the file, so there is no second copy.
    pub fn open_read_only(id: DocumentId, path: PathBuf) -> Result<Self> {
        let metadata = std::fs::metadata(&path).map_err(|error| Error::io("read", &path, error))?;
        if metadata.len() > MAX_READ_ONLY_BYTES {
            return Err(Error::FileTooLarge {
                path,
                size: metadata.len(),
                limit: MAX_READ_ONLY_BYTES,
            });
        }
        let file = std::fs::File::open(&path).map_err(|error| Error::io("read", &path, error))?;
        let rope = Rope::from_reader(file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                Error::InvalidUtf8 { path: path.clone() }
            } else {
                Error::io("read", &path, error)
            }
        })?;
        let eol = detect_rope_eol(&rope);
        let mut document = Self {
            id,
            path: Some(path),
            rope,
            dirty: false,
            read_only: true,
            eol,
            indent: crate::editorconfig::IndentSettings::default(),
            history: History::default(),
            folded: Vec::new(),
            syntax: RefCell::new(SyntaxCache::default()),
            revision: 0,
            disk_modified: None,
            disk_len: 0,
        };
        document.record_disk_state();
        Ok(document)
    }

    /// True for files opened through [`open_read_only`](Self::open_read_only).
    pub fn is_read_only(&self) -> bool {
        self.read_only
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

    /// Length of `line` without its terminator, without materializing it.
    pub fn line_len(&self, line: usize) -> usize {
        let slice = self
            .rope
            .line(line.min(self.line_count().saturating_sub(1)));
        let mut len = slice.len_chars();
        while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
            len -= 1;
        }
        len
    }

    /// Line terminator used for newly inserted lines.
    pub fn line_ending(&self) -> &'static str {
        self.eol.ending()
    }

    /// True when `line` sits inside a closed fold (the header stays visible).
    pub fn is_hidden(&self, line: usize) -> bool {
        self.folded
            .iter()
            .any(|range| line > range.start && line <= range.end)
    }

    /// Open every fold that starts at or covers `line`.
    pub fn unfold_line(&mut self, line: usize) -> bool {
        let before = self.folded.len();
        self.folded
            .retain(|range| range.start != line && !(line > range.start && line <= range.end));
        self.folded.len() != before
    }

    /// Re-anchor closed folds across `start..end` replaced by `inserted`.
    ///
    /// An edit entirely above a fold shifts it, an edit in a fold header keeps
    /// it closed with a shifted body, and an edit that touches the hidden body
    /// opens only that fold. Line numbers below the edit move by the difference
    /// between inserted and removed line breaks.
    fn reanchor_folds(&mut self, start: usize, end: usize, inserted: &str) {
        if self.folded.is_empty() {
            return;
        }
        let start = start.min(self.len_chars());
        let end = end.min(self.len_chars()).max(start);
        let start_line = self.line_of_char(start);
        let end_line = self.line_of_char(end);
        let added = inserted.bytes().filter(|byte| *byte == b'\n').count() as isize;
        let removed = end_line.saturating_sub(start_line) as isize;
        let delta = added - removed;
        let chars = self.len_chars();
        let folds = std::mem::take(&mut self.folded);
        let line_start = |line: usize| {
            if line >= self.line_count() {
                chars
            } else {
                self.char_of_line(line)
            }
        };
        let mut kept = Vec::with_capacity(folds.len());
        for range in folds {
            let header = line_start(range.start);
            let body = line_start(range.start + 1);
            let after = line_start(range.end + 1);
            if end <= header {
                // Above the header: the whole fold moves.
                let moved = range
                    .start
                    .checked_add_signed(delta)
                    .zip(range.end.checked_add_signed(delta));
                if let Some((start, end)) = moved {
                    kept.push(start..end);
                }
            } else if start >= after {
                // Below the fold: nothing moves.
                kept.push(range);
            } else if end <= body {
                // Header-only edit: the body shifts with the line count.
                if let Some(end) = range.end.checked_add_signed(delta)
                    && end >= range.start
                {
                    kept.push(range.start..end);
                }
            }
            // Otherwise the edit touches hidden lines; the fold opens.
        }
        self.folded = kept;
    }

    /// Replace the fold set from a session, clamped to the current line count.
    pub fn restore_folds(&mut self, ranges: impl IntoIterator<Item = Range<usize>>) {
        let last = self.line_count().saturating_sub(1);
        let mut folds: Vec<Range<usize>> = ranges
            .into_iter()
            .filter_map(|range| {
                let start = range.start.min(last);
                let end = range.end.min(last);
                (start < end).then_some(start..end)
            })
            .collect();
        folds.sort_by_key(|range| (range.start, range.end));
        folds.dedup();
        self.folded = folds;
    }

    /// Toggle the innermost fold at `line`. Returns the number of hidden lines
    /// when it folds, `Some(0)` when it unfolds, and `None` when nothing is
    /// foldable there.
    pub fn toggle_fold(&mut self, line: usize) -> Option<usize> {
        if self.read_only {
            return None;
        }
        if self
            .folded
            .iter()
            .any(|range| range.start == line || (line > range.start && line <= range.end))
        {
            self.unfold_line(line);
            return Some(0);
        }
        let language = Language::from_path(self.path.as_deref());
        let range = crate::structure::folds(&self.rope, language)
            .into_iter()
            .filter(|range| range.start == line)
            .min_by_key(|range| range.end)?;
        let hidden = range.end - range.start;
        self.folded.push(range);
        Some(hidden)
    }

    /// Highlight spans for `line`, using the cached lexer state from the lines
    /// above it. Filling the cache is derived work: rendering can call this
    /// through `&self` without changing the document.
    pub fn syntax_spans(&self, line: usize, text: &str) -> Vec<Span> {
        if self.read_only {
            return Vec::new();
        }
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
        if self.read_only {
            return vec![Vec::new(); count];
        }
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
        if self.read_only {
            return;
        }
        let char_idx = char_idx.min(self.len_chars());
        let line = self.line_of_char(char_idx);
        let edit = self.input_edit(char_idx, char_idx, text);
        self.reanchor_folds(char_idx, char_idx, text);
        self.rope.insert(char_idx, text);
        self.history
            .record_insert(char_idx, text, cursor_before, scroll_before);
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
        self.syntax.get_mut().tree.apply_edit(edit);
        self.touch();
    }

    pub fn remove(
        &mut self,
        range: Range<usize>,
        cursor_before: usize,
        scroll_before: (usize, u16),
    ) -> String {
        if self.read_only {
            return String::new();
        }
        let start = range.start.min(self.len_chars());
        let end = range.end.min(self.len_chars());
        if start >= end {
            return String::new();
        }
        let line = self.line_of_char(start);
        let text: String = self.rope.slice(start..end).to_string();
        let edit = self.input_edit(start, end, "");
        self.reanchor_folds(start, end, "");
        self.rope.remove(start..end);
        self.history
            .record_remove(start, &text, cursor_before, scroll_before);
        self.dirty = self.history.is_modified();
        self.invalidate_syntax(line);
        self.syntax.get_mut().tree.apply_edit(edit);
        self.touch();
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
        if self.read_only {
            return;
        }
        let start = range.start.min(self.len_chars());
        let end = range.end.min(self.len_chars());
        if start >= end {
            self.insert(start, text, cursor_before, scroll_before);
            return;
        }
        let line = self.line_of_char(start);
        let removed: String = self.rope.slice(start..end).to_string();
        let edit = self.input_edit(start, end, text);
        self.reanchor_folds(start, end, text);
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
        self.touch();
    }

    /// Returns `(cursor, scroll)` to restore on the pane that issued the edit.
    pub fn undo(&mut self) -> Option<(usize, (usize, u16))> {
        if self.read_only {
            return None;
        }
        let (restore, eol) = self.history.undo(&mut self.rope)?;
        if let Some(eol) = eol {
            self.eol = eol;
        }
        self.dirty = self.history.is_modified();
        self.folded.clear();
        self.reset_syntax();
        self.touch();
        Some(restore)
    }

    pub fn redo(&mut self) -> Option<(usize, (usize, u16))> {
        if self.read_only {
            return None;
        }
        let (restore, eol) = self.history.redo(&mut self.rope)?;
        if let Some(eol) = eol {
            self.eol = eol;
        }
        self.dirty = self.history.is_modified();
        self.folded.clear();
        self.reset_syntax();
        self.touch();
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
        if self.read_only {
            return Err(Error::ReadOnly(format!(
                "{} is read-only (larger than {} MiB)",
                path.display(),
                MAX_FILE_BYTES >> 20
            )));
        }
        let text = self.rope.to_string();
        write_atomic(&path, text.as_bytes()).map_err(|error| Error::io("write", &path, error))?;
        self.path = Some(path);
        // Typing that lands within the coalescing window after a save would
        // merge into the batch just marked as saved, leaving the buffer clean
        // with unsaved text in it.
        self.history.break_run();
        self.history.mark_saved();
        self.dirty = false;
        self.record_disk_state();
        Ok(())
    }

    /// Replace the buffer with recovered text, dropping undo history.
    pub fn restore(&mut self, text: &str) {
        if self.read_only {
            return;
        }
        self.rope = Rope::from_str(text);
        self.history = History::default();
        self.dirty = true;
        self.folded.clear();
        self.reset_syntax();
        self.touch();
        self.record_disk_state();
    }

    /// Replace several non-overlapping ranges as one undo step. Ranges must be
    /// ascending; they are applied from the end so earlier offsets stay valid.
    pub fn replace_ranges(
        &mut self,
        ranges: &[Range<usize>],
        text: &str,
        cursor_before: usize,
        cursor_after: usize,
        scroll_before: (usize, u16),
    ) -> bool {
        if self.read_only {
            return false;
        }
        if let [range] = ranges {
            if range.is_empty() {
                if text.is_empty() {
                    return false;
                }
                self.insert(range.start, text, cursor_before, scroll_before);
                return true;
            }
            if text.is_empty() {
                self.remove(range.clone(), cursor_before, scroll_before);
                return true;
            }
            self.replace(
                range.clone(),
                text,
                cursor_before,
                cursor_after,
                scroll_before,
            );
            return true;
        }
        let mut edits = Vec::new();
        let inserted = text.chars().count();
        for range in ranges.iter().rev() {
            let start = range.start.min(self.len_chars());
            let end = range.end.min(self.len_chars());
            if start < end {
                let removed: String = self.rope.slice(start..end).to_string();
                self.reanchor_folds(start, end, "");
                self.rope.remove(start..end);
                edits.push(Edit::Remove {
                    char_idx: start,
                    text: removed,
                });
            }
            if inserted > 0 && start <= self.len_chars() {
                self.reanchor_folds(start, start, text);
                self.rope.insert(start, text);
                edits.push(Edit::Insert {
                    char_idx: start,
                    text: text.to_string(),
                });
            }
        }
        if edits.is_empty() {
            return false;
        }
        self.history
            .record_edits(edits, cursor_before, cursor_after, scroll_before);
        self.dirty = self.history.is_modified();
        self.reset_syntax();
        self.touch();
        true
    }

    /// Convert every line ending to `target` as one undo step. Returns whether
    /// the buffer changed. New lines use the new ending afterwards.
    pub fn convert_eol(&mut self, target: Eol) -> bool {
        if self.read_only {
            return false;
        }
        let text = self.rope.to_string();
        let lf = text.replace("\r\n", "\n").replace('\r', "\n");
        let converted = match target {
            Eol::Lf => lf,
            Eol::Crlf => lf.replace('\n', "\r\n"),
        };
        let before = self.eol;
        self.eol = target;
        if converted == text {
            return false;
        }
        let end = self.len_chars();
        let range = 0..end;
        self.replace_ranges(std::slice::from_ref(&range), &converted, 0, 0, (0, 0));
        self.history.tag_eol(before, target);
        true
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
        if self.read_only {
            return Err(Error::ReadOnly(format!(
                "{} is read-only (larger than {} MiB)",
                self.path
                    .as_deref()
                    .unwrap_or(Path::new("document"))
                    .display(),
                MAX_FILE_BYTES >> 20
            )));
        }
        let path = self.path.clone().ok_or(Error::NoPath)?;
        let text = read_text(&path)?;
        self.eol = Eol::detect(&text);
        self.rope = Rope::from_str(&text);
        self.history = History::default();
        self.dirty = false;
        self.folded.clear();
        self.reset_syntax();
        self.touch();
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

    /// Record the disk state `text` was based on, so a later save can tell
    /// whether the file moved on while the buffer was dirty.
    pub(crate) fn record_disk_stamp(&mut self, modified: Option<SystemTime>, len: u64) {
        self.disk_modified = modified;
        self.disk_len = len;
    }

    /// Mark the rope as changed for observers that track `revision`.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Replace `path` without ever truncating the original: write an unpredictable
/// sibling temp file, flush it, rename it over the target, then flush the
/// directory so the rename survives a crash. Symlinks are followed so the link
/// survives. Existing permissions are preserved. A failed rename is reported;
/// the original file is left untouched.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let permissions = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let mut temporary = tempfile::Builder::new()
        .prefix(".i3-save-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().set_permissions(match permissions {
        Some(permissions) => permissions,
        #[cfg(unix)]
        None => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(default_file_mode(parent))
        }
        #[cfg(not(unix))]
        None => std::fs::metadata(&temporary)?.permissions(),
    })?;
    temporary.as_file().sync_all()?;
    if let Err(error) = temporary.persist(&target) {
        return Err(std::io::Error::new(
            error.error.kind(),
            format!("replace {}: {}", target.display(), error.error),
        ));
    }
    sync_dir(parent);
    Ok(())
}

/// Flush a directory entry so a rename survives a crash. Best effort: some
/// filesystems reject `fsync` on a directory handle.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Mode a new file gets in `dir`: ask the kernel with a throwaway probe so the
/// process umask decides, instead of a hard-coded 0644.
#[cfg(unix)]
fn default_file_mode(dir: &Path) -> u32 {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let probe = dir.join(format!(
        ".i3-mode-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    let Ok(file) = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o666)
        .open(&probe)
    else {
        return 0o644;
    };
    let mode = file
        .metadata()
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(0o644);
    drop(file);
    let _ = std::fs::remove_file(&probe);
    mode
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
    fn save_preserves_a_lone_carriage_return() {
        let dir = std::env::temp_dir().join(format!("i3-editor-cr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("progress.txt");
        std::fs::write(&path, "progress 10%\rprogress 100%\n").expect("seed");

        let mut document = Document::open(DocumentId(1), path.clone()).expect("open");
        assert_eq!(document.line_count(), 3);
        assert_eq!(document.line_text(0), "progress 10%");
        assert_eq!(document.line_text(1), "progress 100%");

        document.insert(0, "X", 0, (0, 0));
        document.save().expect("save");
        assert_eq!(
            std::fs::read(&path).expect("read"),
            b"Xprogress 10%\rprogress 100%\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn typing_right_after_a_save_still_counts_as_unsaved() {
        let dir = std::env::temp_dir().join(format!("i3-editor-aftersave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("note.txt");
        std::fs::write(&path, "one\n").expect("seed");

        let mut document = Document::open(DocumentId(1), path.clone()).expect("open");
        document.insert(0, "a", 0, (0, 0));
        document.save().expect("save");
        assert!(!document.dirty, "just saved");
        // Same coalescing window as the saved batch: this must not merge with it.
        document.insert(1, "b", 1, (0, 0));
        assert!(document.dirty, "the edit after the save is unsaved");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undoing_an_eol_conversion_restores_the_old_mode() {
        let mut document = Document::from_text(DocumentId(1), "a\nb\n", None);
        assert_eq!(document.eol, Eol::Lf);
        assert!(document.convert_eol(Eol::Crlf));
        assert_eq!(document.eol, Eol::Crlf);
        assert!(document.undo().is_some());
        assert_eq!(document.eol, Eol::Lf);
        assert_eq!(document.rope.to_string(), "a\nb\n");
        std::fs::remove_dir_all(std::env::temp_dir()).ok();
    }

    #[test]
    fn mixed_line_endings_are_not_normalized() {
        let dir = std::env::temp_dir().join(format!("i3-editor-mixed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("mixed.txt");
        std::fs::write(&path, "a\r\nb\nc\r\n").expect("seed");

        let mut document = Document::open(DocumentId(1), path.clone()).expect("open");
        assert_eq!(document.eol, Eol::Crlf);
        document.insert(0, "X", 0, (0, 0));
        document.save().expect("save");
        assert_eq!(std::fs::read(&path).expect("read"), b"Xa\r\nb\nc\r\n");
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
    #[cfg(unix)]
    fn a_new_file_takes_its_mode_from_the_umask() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let dir = std::env::temp_dir().join(format!("i3-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let reference = dir.join("reference");
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o666)
            .open(&reference)
            .expect("reference");
        let expected = std::fs::metadata(&reference)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;

        let target = dir.join("fresh.txt");
        write_atomic(&target, b"x").expect("write");
        let actual = std::fs::metadata(&target)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(actual, expected, "new files must follow the process umask");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn line_len_matches_the_stripped_line_text() {
        for text in ["a\nb\n", "a\r\nb\r\n", "no-newline", "ünïcode\n\r\n"] {
            let document = Document::from_text(DocumentId(1), text, None);
            for line in 0..document.line_count() {
                assert_eq!(
                    document.line_len(line),
                    document.line_text(line).chars().count(),
                    "{text:?} line {line}"
                );
            }
        }
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

    #[cfg(unix)]
    #[test]
    fn save_ignores_a_prepared_temp_symlink() {
        let dir = std::env::temp_dir().join(format!("i3-save-symlink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let target = dir.join("doc.txt");
        std::fs::write(&target, "old\n").expect("seed");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "victim\n").expect("victim");
        let predicted = dir.join(format!(".doc.txt.i3-{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &predicted).expect("symlink");

        let mut document = doc("new\n");
        document.save_as(target.clone()).expect("save");
        assert_eq!(
            std::fs::read_to_string(&victim).expect("victim"),
            "victim\n"
        );
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "new\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folds_shift_with_edits_above_them() {
        let mut document = doc("a\nfn alpha() {\n    one();\n}\n");
        document.restore_folds(std::iter::once(1..3));
        document.insert(0, "// hi\n", 0, (0, 0));
        assert_eq!(document.folded, vec![2..4]);
        assert!(document.is_hidden(3));
    }

    #[test]
    fn editing_a_fold_header_keeps_it_closed() {
        let mut document = doc("fn alpha() {\n    one();\n    two();\n}\n");
        document.restore_folds(std::iter::once(0..3));
        document.insert(3, "x", 3, (0, 0));
        assert_eq!(document.folded, vec![0..3]);
        assert!(document.is_hidden(1));
    }

    #[test]
    fn editing_a_fold_body_opens_only_that_fold() {
        let mut document = doc("fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n");
        document.folded = vec![0..2, 4..6];
        let index = document.char_of_line(1) + 4;
        document.insert(index, "x", index, (0, 0));
        assert_eq!(document.folded, vec![4..6]);
        assert!(document.is_hidden(5));
    }

    #[test]
    fn read_only_documents_refuse_mutation_and_saving() {
        let dir = std::env::temp_dir().join(format!("i3-readonly-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("large.txt");
        std::fs::write(&path, "a\r\nb\r\n").expect("seed");

        let mut document = Document::open_read_only(DocumentId(1), path.clone()).expect("open");
        assert!(document.is_read_only());
        assert_eq!(document.eol, Eol::Crlf);
        document.insert(0, "x", 0, (0, 0));
        document.remove(0..1, 0, (0, 0));
        assert_eq!(document.rope.to_string(), "a\r\nb\r\n");
        assert!(document.save().is_err());
        assert!(document.syntax_spans(0, "a").is_empty());
        assert!(document.syntax_spans_range(0, 2).iter().all(Vec::is_empty));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restored_folds_are_clamped_to_the_file() {
        let mut document = doc("one\ntwo");
        document.restore_folds(vec![0..9, 1..1]);
        assert_eq!(document.folded, vec![0..1]);
    }

    #[test]
    fn toggle_fold_uses_tree_sitter_ranges() {
        let mut document = doc("fn alpha() {\n    one();\n}\n\nafter\n");
        document.path = Some(PathBuf::from("/tmp/note.rs"));
        assert_eq!(document.toggle_fold(0), Some(2));
        assert!(document.is_hidden(1));
        assert!(document.is_hidden(2));
        assert!(!document.is_hidden(3));
        assert!(!document.is_hidden(4));
        assert_eq!(document.toggle_fold(0), Some(0));
        assert!(!document.is_hidden(1));
        assert_eq!(document.toggle_fold(4), None);
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
