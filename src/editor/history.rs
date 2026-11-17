//! Undo history: an operation log, because Ropey has none.

use std::time::{Duration, Instant};

use ropey::Rope;

/// Typing inside this window merges into one undo step.
pub const COALESCE: Duration = Duration::from_millis(300);

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
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Batch>,
    redo: Vec<Batch>,
    last_edit: Option<Instant>,
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
            && self.undo.last().is_some_and(|batch| {
                batch.edits.iter().all(Edit::is_insert)
                    && batch.edits.last().is_some_and(|edit| match edit {
                        Edit::Insert { text, .. } => !text.contains('\n') && !text.contains('\r'),
                        Edit::Remove { .. } => false,
                    })
                    && batch.edits.last().and_then(Edit::insert_end) == Some(char_idx)
            });
        if merges {
            if let Some(batch) = self.undo.last_mut() {
                batch.edits.push(Edit::Insert {
                    char_idx,
                    text: text.to_string(),
                });
                batch.cursor_after = end;
            }
        } else {
            self.undo.push(Batch {
                edits: vec![Edit::Insert {
                    char_idx,
                    text: text.to_string(),
                }],
                cursor_before,
                cursor_after: end,
                scroll_before,
            });
            self.redo.clear();
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
        self.undo.push(Batch {
            edits: vec![Edit::Remove {
                char_idx,
                text: text.to_string(),
            }],
            cursor_before,
            cursor_after: char_idx,
            scroll_before,
        });
        self.redo.clear();
        self.last_edit = None;
    }

    /// Returns the cursor and scroll to restore on the pane that issued the edit.
    pub fn undo(&mut self, rope: &mut Rope) -> Option<(usize, (usize, u16))> {
        let batch = self.undo.pop()?;
        for edit in batch.edits.iter().rev() {
            edit.revert(rope);
        }
        let restore = (batch.cursor_before, batch.scroll_before);
        self.redo.push(batch);
        self.last_edit = None;
        Some(restore)
    }

    pub fn redo(&mut self, rope: &mut Rope) -> Option<(usize, (usize, u16))> {
        let batch = self.redo.pop()?;
        for edit in &batch.edits {
            edit.apply(rope);
        }
        let restore = (batch.cursor_after, batch.scroll_before);
        self.undo.push(batch);
        self.last_edit = None;
        Some(restore)
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
