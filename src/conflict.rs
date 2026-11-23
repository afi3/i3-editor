//! Git conflict markers: locate blocks and choose a side.

use crate::editor::document::Document;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub start_line: usize,
    pub separator_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    Both,
}

impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Choice::Ours => "ours",
            Choice::Theirs => "theirs",
            Choice::Both => "both",
        }
    }
}

/// Conflict blocks in document order. Incomplete marker groups are ignored.
pub fn blocks(document: &Document) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut start = None;
    let mut separator = None;
    for line in 0..document.line_count() {
        let text = document.line_text(line);
        if text.starts_with("<<<<<<<") {
            start = Some(line);
            separator = None;
        } else if start.is_some() && separator.is_none() && text.starts_with("=======") {
            separator = Some(line);
        } else if let (Some(start_line), Some(separator_line)) = (start, separator)
            && text.starts_with(">>>>>>>")
        {
            blocks.push(Block {
                start_line,
                separator_line,
                end_line: line,
            });
            start = None;
            separator = None;
        }
    }
    blocks
}

/// The block containing `line`, otherwise the first block after it.
pub fn at_or_after(blocks: &[Block], line: usize) -> Option<Block> {
    blocks
        .iter()
        .copied()
        .find(|block| block.start_line <= line && line <= block.end_line)
        .or_else(|| {
            blocks
                .iter()
                .copied()
                .find(|block| block.start_line >= line)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DocumentId;

    fn document(text: &str) -> Document {
        let mut document = Document::empty(DocumentId(1), None);
        document.rope = ropey::Rope::from_str(text);
        document
    }

    #[test]
    fn finds_each_complete_block_and_ignores_partial_ones() {
        let document = document(
            "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nafter\n\
             <<<<<<< HEAD\nincomplete\n",
        );
        let blocks = blocks(&document);
        assert_eq!(
            blocks,
            vec![Block {
                start_line: 1,
                separator_line: 3,
                end_line: 5
            }]
        );
        assert_eq!(at_or_after(&blocks, 0), Some(blocks[0]));
        assert_eq!(at_or_after(&blocks, 4), Some(blocks[0]));
        assert_eq!(at_or_after(&blocks, 6), None);
    }
}
