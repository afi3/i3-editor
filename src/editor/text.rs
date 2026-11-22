//! Text helpers shared by cursor motion and rendering.
//!
//! The buffer counts chars; the terminal counts cells. Every conversion lives
//! here so the two never get mixed in a command handler.

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A grapheme cluster can be long, but a ZWJ sequence still fits inside this
/// window. It keeps the common path free of per-neighbour allocation.
const GRAPHEME_WINDOW: usize = 64;

pub fn next_grapheme(rope: &Rope, char_idx: usize) -> usize {
    let len = rope.len_chars();
    if char_idx >= len {
        return len;
    }
    let end = (char_idx + GRAPHEME_WINDOW).min(len);
    let text = rope.slice(char_idx..end).to_string();
    let width = text
        .graphemes(true)
        .next()
        .map(|grapheme| grapheme.chars().count())
        .unwrap_or(1);
    (char_idx + width).min(len)
}

pub fn prev_grapheme(rope: &Rope, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    let start = char_idx.saturating_sub(GRAPHEME_WINDOW);
    let text = rope.slice(start..char_idx).to_string();
    let width = text
        .graphemes(true)
        .next_back()
        .map(|grapheme| grapheme.chars().count())
        .unwrap_or(1);
    char_idx.saturating_sub(width)
}

/// Skip whitespace, then the adjacent word run. Used by Ctrl+Left / Ctrl+Right.
pub fn prev_word(rope: &Rope, char_idx: usize) -> usize {
    let mut idx = char_idx;
    while idx > 0 {
        let prev = prev_grapheme(rope, idx);
        if !rope.char(prev).is_whitespace() {
            break;
        }
        idx = prev;
    }
    while idx > 0 {
        let prev = prev_grapheme(rope, idx);
        if rope.char(prev).is_whitespace() {
            break;
        }
        idx = prev;
    }
    idx
}

/// Matching bracket for the bracket at, or immediately before, the cursor.
/// Scanning is bounded so a pathological file cannot stall a frame.
pub fn matching_bracket(rope: &Rope, cursor: usize) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    if len == 0 {
        return None;
    }
    let position = if is_bracket(rope.char(cursor.min(len - 1))) {
        cursor.min(len - 1)
    } else if cursor > 0 && is_bracket(rope.char(cursor - 1)) {
        cursor - 1
    } else {
        return None;
    };
    match rope.char(position) {
        '(' => scan_forward(rope, position, '(', ')'),
        ')' => scan_backward(rope, position, '(', ')'),
        '[' => scan_forward(rope, position, '[', ']'),
        ']' => scan_backward(rope, position, '[', ']'),
        '{' => scan_forward(rope, position, '{', '}'),
        '}' => scan_backward(rope, position, '{', '}'),
        _ => None,
    }
}

const BRACKET_SCAN_LIMIT: usize = 10_000;

fn scan_forward(rope: &Rope, position: usize, open: char, close: char) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let end = (position + 1 + BRACKET_SCAN_LIMIT).min(rope.len_chars());
    for index in position + 1..end {
        let ch = rope.char(index);
        if ch == open {
            depth += 1;
        } else if ch == close {
            if depth == 0 {
                return Some((position, index));
            }
            depth -= 1;
        }
    }
    None
}

fn scan_backward(rope: &Rope, position: usize, open: char, close: char) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let start = position.saturating_sub(BRACKET_SCAN_LIMIT);
    for index in (start..position).rev() {
        let ch = rope.char(index);
        if ch == close {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                return Some((index, position));
            }
            depth -= 1;
        }
    }
    None
}

fn is_bracket(ch: char) -> bool {
    matches!(ch, '(' | ')' | '[' | ']' | '{' | '}')
}

pub fn next_word(rope: &Rope, char_idx: usize) -> usize {
    let len = rope.len_chars();
    let mut idx = char_idx;
    while idx < len {
        if !rope.char(idx).is_whitespace() {
            break;
        }
        idx = next_grapheme(rope, idx);
    }
    while idx < len {
        if rope.char(idx).is_whitespace() {
            break;
        }
        idx = next_grapheme(rope, idx);
    }
    idx
}

/// Display cells for a text fragment, expanding tabs to the next tab stop.
pub fn cells(text: &str, tab_width: usize, start_cell: u16) -> u16 {
    let mut cell = start_cell;
    for ch in text.chars() {
        if ch == '\t' {
            let tab = tab_width.max(1) as u16;
            cell += tab - (cell % tab);
        } else {
            cell += UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4])) as u16;
        }
    }
    cell - start_cell
}

/// Cells occupied by the first `char_count` chars of `text`.
pub fn cells_prefix(text: &str, char_count: usize, tab_width: usize) -> u16 {
    let prefix: String = text.chars().take(char_count).collect();
    cells(&prefix, tab_width, 0)
}

/// Char index whose display cell contains `target`, clamped to the line end.
pub fn char_at_cell(text: &str, target: u16, tab_width: usize) -> usize {
    let tab = tab_width.max(1) as u16;
    let mut cell = 0u16;
    for (index, ch) in text.chars().enumerate() {
        let advance = if ch == '\t' {
            tab - (cell % tab)
        } else {
            UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4])) as u16
        };
        if target < cell.saturating_add(advance) {
            // Clicking inside a tab or a wide glyph lands after it; clicking the
            // exact first cell lands before it.
            return if target > cell { index + 1 } else { index };
        }
        cell = cell.saturating_add(advance);
    }
    text.chars().count()
}

/// Render text into a flat string with tabs expanded, dropping the cells before
/// `skip` and keeping at most `width` cells.
pub fn clip(text: &str, skip: u16, width: u16, tab_width: usize) -> String {
    let mut out = String::new();
    let mut cell = 0u16;
    let limit = skip.saturating_add(width);
    for ch in text.chars() {
        let advance: u16 = if ch == '\t' {
            let tab = tab_width.max(1) as u16;
            tab - (cell % tab)
        } else {
            UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4])) as u16
        };
        let next = cell + advance;
        if next > skip && cell < limit {
            if ch == '\t' {
                for _ in cell.max(skip)..next.min(limit) {
                    out.push(' ');
                }
            } else {
                out.push(ch);
            }
        }
        cell = next;
        if cell >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn grapheme_motion_skips_combining_marks() {
        let r = rope("ae\u{301}b");
        assert_eq!(next_grapheme(&r, 0), 1);
        assert_eq!(next_grapheme(&r, 1), 3);
        assert_eq!(prev_grapheme(&r, 3), 1);
        assert_eq!(prev_grapheme(&r, 1), 0);
    }

    #[test]
    fn grapheme_motion_handles_zwj_sequences() {
        let r = rope("\u{1f469}\u{200d}\u{1f4bb}x");
        assert_eq!(next_grapheme(&r, 0), 3);
        assert_eq!(prev_grapheme(&r, 3), 0);
    }

    #[test]
    fn word_motion_stops_at_word_boundaries() {
        let r = rope("alpha beta  gamma");
        assert_eq!(next_word(&r, 0), 5);
        assert_eq!(next_word(&r, 5), 10);
        assert_eq!(prev_word(&r, 10), 6);
        assert_eq!(prev_word(&r, 6), 0);
        assert_eq!(prev_word(&r, 5), 0);
    }

    #[test]
    fn cells_expand_tabs_to_stops() {
        assert_eq!(cells("a", 4, 0), 1);
        assert_eq!(cells("\t", 4, 0), 4);
        assert_eq!(cells("\t", 4, 1), 3);
        assert_eq!(cells("ab\t", 4, 0), 4);
    }

    #[test]
    fn cells_count_wide_characters_as_two() {
        assert_eq!(cells("\u{4f60}\u{597d}", 4, 0), 4);
    }

    #[test]
    fn char_at_cell_matches_display_widths() {
        assert_eq!(char_at_cell("abcdef", 0, 4), 0);
        assert_eq!(char_at_cell("abcdef", 3, 4), 3);
        assert_eq!(char_at_cell("abcdef", 99, 4), 6);
        assert_eq!(char_at_cell("\tab", 2, 4), 1);
        assert_eq!(char_at_cell("\u{4f60}\u{597d}", 1, 4), 1);
        assert_eq!(char_at_cell("\u{4f60}\u{597d}", 2, 4), 1);
        assert_eq!(char_at_cell("\u{4f60}\u{597d}", 3, 4), 2);
    }

    #[test]
    fn clip_respects_skip_and_width() {
        assert_eq!(clip("abcdef", 2, 3, 4), "cde");
        assert_eq!(clip("\tab", 1, 2, 4), "  ");
        assert_eq!(clip("abc", 5, 3, 4), "");
    }

    #[test]
    fn matching_bracket_handles_nesting_and_both_sides() {
        let rope = Rope::from_str("f(a(b)c)d");
        assert_eq!(matching_bracket(&rope, 1), Some((1, 7)));
        assert_eq!(matching_bracket(&rope, 5), Some((3, 5)));
        assert_eq!(matching_bracket(&rope, 2), Some((1, 7)));
        assert_eq!(matching_bracket(&rope, 0), None);
        assert_eq!(matching_bracket(&Rope::from_str("("), 0), None);
    }
}
