//! Undo history: an operation log, because Ropey has none.
//!
//! Batches form a chain (`id`/`parent`) so the document can tell whether the
//! current position matches the last saved position, even after undo/redo.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ropey::Rope;

/// Typing inside this window merges into one undo step.
pub const COALESCE: Duration = Duration::from_millis(300);

/// Oldest batches are dropped beyond this depth so long sessions stay bounded.
pub const MAX_BATCHES: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Insert { char_idx: usize, text: String },
    Remove { char_idx: usize, text: String },
}

impl Edit {
    pub fn apply(&self, rope: &mut Rope) {
        match self {
            Edit::Insert { char_idx, text } => rope.insert(*char_idx, text),
            Edit::Remove { char_idx, text } => {
                rope.remove(*char_idx..char_idx + text.chars().count());
            }
        }
    }

    pub fn revert(&self, rope: &mut Rope) {
        match self {
            Edit::Insert { char_idx, text } => {
                rope.remove(*char_idx..char_idx + text.chars().count());
            }
            Edit::Remove { char_idx, text } => rope.insert(*char_idx, text),
        }
    }

    pub fn is_insert(&self) -> bool {
        matches!(self, Edit::Insert { .. })
    }

    pub fn insert_end(&self) -> Option<usize> {
        match self {
            Edit::Insert { char_idx, text } => Some(char_idx + text.chars().count()),
            Edit::Remove { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub edits: Vec<Edit>,
    pub cursor_before: usize,
    pub cursor_after: usize,
    pub scroll_before: (usize, u16),
    /// Revision this batch produced.
    pub id: u64,
    /// Revision this batch started from; restored on undo.
    pub parent: u64,
}

#[derive(Debug)]
pub struct History {
    undo: VecDeque<Batch>,
    redo: VecDeque<Batch>,
    next_id: u64,
    current: u64,
    saved: u64,
    last_edit: Option<Instant>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            next_id: 1,
            current: 0,
            saved: 0,
            last_edit: None,
        }
    }
}

impl History {
    pub fn record_insert(
        &mut self,
        char_idx: usize,
        text: &str,
        cursor_before: usize,
        scroll_before: (usize, u16),
    ) {
        let end = char_idx + text.chars().count();
        let merges = self.last_edit.is_some_and(|last| last.elapsed() < COALESCE)
            && !text.contains('\n')
            && !text.contains('\r')
            && self.undo.back().is_some_and(|batch| {
                batch.edits.iter().all(Edit::is_insert)
                    && batch.edits.last().is_some_and(|edit| match edit {
                        Edit::Insert { text, .. } => !text.contains('\n') && !text.contains('\r'),
                        Edit::Remove { .. } => false,
                    })
                    && batch.edits.last().and_then(Edit::insert_end) == Some(char_idx)
            });
        if merges {
            if let Some(batch) = self.undo.back_mut() {
                batch.edits.push(Edit::Insert {
                    char_idx,
                    text: text.to_string(),
                });
                batch.cursor_after = end;
            }
        } else {
            self.push_batch(Batch {
                edits: vec![Edit::Insert {
                    char_idx,
                    text: text.to_string(),
                }],
                cursor_before,
                cursor_after: end,
                scroll_before,
                id: self.next_id,
                parent: self.current,
            });
        }
        self.last_edit = Some(Instant::now());
    }

    pub fn record_remove(
        &mut self,
        char_idx: usize,
        text: &str,
        cursor_before: usize,
        scroll_before: (usize, u16),
    ) {
        self.push_batch(Batch {
            edits: vec![Edit::Remove {
                char_idx,
                text: text.to_string(),
            }],
            cursor_before,
            cursor_after: char_idx,
            scroll_before,
            id: self.next_id,
            parent: self.current,
        });
        self.last_edit = None;
    }

    fn push_batch(&mut self, batch: Batch) {
        self.next_id = self.next_id.saturating_add(1);
        self.current = batch.id;
        self.undo.push_back(batch);
        while self.undo.len() > MAX_BATCHES {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    /// Returns the cursor and scroll to restore on the pane that issued the edit.
    pub fn undo(&mut self, rope: &mut Rope) -> Option<(usize, (usize, u16))> {
        let batch = self.undo.pop_back()?;
        for edit in batch.edits.iter().rev() {
            edit.revert(rope);
        }
        let restore = (batch.cursor_before, batch.scroll_before);
        self.current = batch.parent;
        self.redo.push_back(batch);
        self.last_edit = None;
        Some(restore)
    }

    pub fn redo(&mut self, rope: &mut Rope) -> Option<(usize, (usize, u16))> {
        let batch = self.redo.pop_back()?;
        for edit in &batch.edits {
            edit.apply(rope);
        }
        let restore = (batch.cursor_after, batch.scroll_before);
        self.current = batch.id;
        self.undo.push_back(batch);
        while self.undo.len() > MAX_BATCHES {
            self.undo.pop_front();
        }
        self.last_edit = None;
        Some(restore)
    }

    /// Mark the current revision as the saved one.
    pub fn mark_saved(&mut self) {
        self.saved = self.current;
    }

    /// True when the buffer content differs from the last saved revision.
    pub fn is_modified(&self) -> bool {
        self.current != self.saved
    }

    /// A cursor move or newline ends the current coalescing run.
    pub fn break_run(&mut self) {
        self.last_edit = None;
    }

    #[cfg(test)]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn saved_revision_survives_undo_and_redo() {
        let mut history = History::default();
        let mut text = rope("a");
        history.record_insert(1, "b", 1, (0, 0));
        text.insert(1, "b");
        assert!(history.is_modified());

        history.mark_saved();
        assert!(!history.is_modified());

        history.undo(&mut text);
        assert!(history.is_modified(), "undoing past the save is a change");
        history.redo(&mut text);
        assert!(!history.is_modified(), "redo returns to the saved revision");
    }

    #[test]
    fn new_edit_after_save_is_modified_again() {
        let mut history = History::default();
        let mut text = rope("a");
        history.record_insert(1, "b", 1, (0, 0));
        text.insert(1, "b");
        history.mark_saved();
        history.record_remove(0, "a", 0, (0, 0));
        text.remove(0..1);
        assert!(history.is_modified());
    }

    #[test]
    fn history_is_capped() {
        let mut history = History::default();
        let mut text = rope("");
        for index in 0..(MAX_BATCHES + 25) {
            let ch = if index % 2 == 0 { "a" } else { "\n" };
            history.record_insert(0, ch, 0, (0, 0));
            text.insert(0, ch);
        }
        assert_eq!(history.undo_depth(), MAX_BATCHES);
    }
}
