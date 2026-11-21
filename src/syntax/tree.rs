//! Structural highlighting with tree-sitter.
//!
//! One parse tree is kept per document. Edits are applied to the tree as
//! [`InputEdit`]s, so the next frame reparses only the changed region. Spans are
//! produced by running the grammar's highlight query over the visible byte range
//! only.

use std::collections::HashMap;
use std::sync::LazyLock;

use ropey::Rope;
use tree_sitter::{Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

use super::{Language, Span, TokenKind};

struct Grammar {
    language: tree_sitter::Language,
    query: Query,
}

static GRAMMARS: LazyLock<HashMap<Language, Grammar>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    let mut add = |language: Language, grammar: tree_sitter::Language, source: &str| {
        if let Ok(query) = Query::new(&grammar, source) {
            map.insert(
                language,
                Grammar {
                    language: grammar,
                    query,
                },
            );
        }
    };

    add(
        Language::Rust,
        tree_sitter_rust::LANGUAGE.into(),
        tree_sitter_rust::HIGHLIGHTS_QUERY,
    );
    add(
        Language::Python,
        tree_sitter_python::LANGUAGE.into(),
        tree_sitter_python::HIGHLIGHTS_QUERY,
    );
    let javascript = format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
    );
    add(
        Language::JavaScript,
        tree_sitter_javascript::LANGUAGE.into(),
        &javascript,
    );
    add(
        Language::TypeScript,
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    );
    add(
        Language::Go,
        tree_sitter_go::LANGUAGE.into(),
        tree_sitter_go::HIGHLIGHTS_QUERY,
    );
    add(
        Language::C,
        tree_sitter_c::LANGUAGE.into(),
        tree_sitter_c::HIGHLIGHT_QUERY,
    );
    add(
        Language::Shell,
        tree_sitter_bash::LANGUAGE.into(),
        tree_sitter_bash::HIGHLIGHT_QUERY,
    );
    map
});

fn grammars() -> &'static HashMap<Language, Grammar> {
    &GRAMMARS
}

/// Whether `language` has a grammar and a compiled highlight query.
pub fn supports(language: Language) -> bool {
    grammars().contains_key(&language)
}

/// The parser state for one document.
#[derive(Default)]
pub(crate) struct Highlighter {
    parser: Option<Parser>,
    tree: Option<Tree>,
    language: Language,
    /// The tree was edited and must be handed back to the parser.
    dirty: bool,
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter")
            .field("language", &self.language)
            .field("parsed", &self.tree.is_some())
            .finish()
    }
}

impl Highlighter {
    /// Drop the tree; the next [`Highlighter::spans`] call parses from scratch.
    pub fn invalidate(&mut self) {
        self.tree = None;
        self.dirty = false;
    }

    /// Keep the tree in sync with an edit that was just applied to the rope.
    pub fn apply_edit(&mut self, edit: tree_sitter::InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(&edit);
            self.dirty = true;
        }
    }

    /// Spans for `count` lines starting at `first`, or `None` when the language
    /// has no grammar or parsing failed.
    pub fn spans(
        &mut self,
        rope: &Rope,
        language: Language,
        first: usize,
        count: usize,
    ) -> Option<Vec<Vec<Span>>> {
        if !self.ensure_tree(rope, language) {
            return None;
        }
        let tree = self.tree.as_ref()?;
        let grammar = grammars().get(&language)?;

        let line_count = rope.len_lines();
        let last = (first + count).min(line_count);
        let count = last.saturating_sub(first);
        if count == 0 {
            return Some(Vec::new());
        }

        let start_byte = rope.line_to_byte(first);
        let end_byte = if last < line_count {
            rope.line_to_byte(last)
        } else {
            rope.len_bytes()
        };

        let mut cells: Vec<Vec<Option<TokenKind>>> = (0..count)
            .map(|offset| vec![None; visible_chars(rope, first + offset)])
            .collect();

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let mut matches = cursor.matches(&grammar.query, tree.root_node(), RopeText { rope });
        while let Some(found) = matches.next() {
            for capture in found.captures() {
                let Some(kind) = token_kind(
                    grammar.query.capture_names()[capture.index as usize],
                    capture.node.kind(),
                ) else {
                    continue;
                };
                paint(rope, capture.node, kind, first, count, &mut cells);
            }
        }

        Some(cells.into_iter().map(|cells| coalesce(&cells)).collect())
    }

    fn ensure_tree(&mut self, rope: &Rope, language: Language) -> bool {
        if self.language != language {
            self.language = language;
            self.tree = None;
            self.dirty = false;
        }
        if self.tree.is_some() && !self.dirty {
            return true;
        }
        let Some(grammar) = grammars().get(&language) else {
            return false;
        };
        let parser = self.parser.get_or_insert_with(Parser::new);
        if parser.set_language(&grammar.language).is_err() {
            return false;
        }
        let len = rope.len_bytes();
        let mut callback = |offset: usize, _: Point| -> &[u8] {
            if offset >= len {
                return &[];
            }
            let (chunk, chunk_byte, _, _) = rope.chunk_at_byte(offset);
            &chunk.as_bytes()[offset - chunk_byte..]
        };
        let old = self.tree.take();
        self.tree = parser.parse_with_options(&mut callback, old.as_ref(), None);
        self.dirty = false;
        self.tree.is_some()
    }
}

/// Text provider for query predicates: yields the rope chunks covering each
/// captured node without flattening the document.
struct RopeText<'a> {
    rope: &'a Rope,
}

impl<'a> tree_sitter::TextProvider<&'a str> for RopeText<'a> {
    type I = ropey::iter::Chunks<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        self.rope.byte_slice(node.byte_range()).chunks()
    }
}

/// Number of chars in a line that are rendered (the terminator is excluded).
fn visible_chars(rope: &Rope, line: usize) -> usize {
    let text = rope.line(line);
    let mut len = text.len_chars();
    while len > 0 {
        let ch = text.char(len - 1);
        if ch == '\n' || ch == '\r' {
            len -= 1;
        } else {
            break;
        }
    }
    len
}

fn paint(
    rope: &Rope,
    node: Node,
    kind: TokenKind,
    first: usize,
    count: usize,
    cells: &mut [Vec<Option<TokenKind>>],
) {
    if node.start_byte() >= node.end_byte() {
        return;
    }
    let first_line = rope.byte_to_line(node.start_byte());
    let last_line = rope.byte_to_line(node.end_byte().saturating_sub(1));
    for line in first_line..=last_line.min(first + count - 1) {
        if line < first {
            continue;
        }
        let line_start = rope.line_to_char(line);
        let len = visible_chars(rope, line);
        if len == 0 {
            continue;
        }
        let line_end = line_start + len;
        let start = rope.byte_to_char(node.start_byte()).max(line_start) - line_start;
        let end = rope.byte_to_char(node.end_byte()).min(line_end) - line_start;
        if start < end && start < len {
            cells[line - first][start..end.min(len)].fill(Some(kind));
        }
    }
}

fn coalesce(cells: &[Option<TokenKind>]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for (index, kind) in cells.iter().enumerate() {
        let Some(kind) = kind else {
            continue;
        };
        match spans.last_mut() {
            Some(last) if last.end == index && last.kind == *kind => last.end = index + 1,
            _ => spans.push(Span {
                start: index,
                end: index + 1,
                kind: *kind,
            }),
        }
    }
    spans
}

/// Map a capture name from a highlights query to a token kind. Captures that
/// name ordinary identifiers (`variable`, `property`, `label`) return `None` so
/// the text keeps the theme foreground.
fn token_kind(capture: &str, node: &str) -> Option<TokenKind> {
    let base = capture.split('.').next().unwrap_or(capture);
    // Rust names numeric literals `constant.builtin`; keep them colored as
    // numbers so a literal looks the same in every language.
    if base == "constant"
        && (node.contains("integer") || node.contains("float") || node == "number")
    {
        return Some(TokenKind::Number);
    }
    Some(match base {
        "keyword" => TokenKind::Keyword,
        "string" | "escape" => TokenKind::String,
        "comment" => TokenKind::Comment,
        "number" | "float" => TokenKind::Number,
        "function" | "constructor" | "method" | "attribute" => TokenKind::Function,
        "type" => TokenKind::TypeName,
        "constant" => TokenKind::Constant,
        "operator" => TokenKind::Operator,
        "punctuation" | "delimiter" => TokenKind::Punctuation,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_language_loads_its_query() {
        for language in [
            Language::Rust,
            Language::Python,
            Language::JavaScript,
            Language::TypeScript,
            Language::Go,
            Language::C,
            Language::Shell,
        ] {
            assert!(supports(language), "{language:?}");
        }
        for language in [Language::Toml, Language::Json, Language::Markdown] {
            assert!(!supports(language), "{language:?} uses the lexer");
        }
    }

    #[test]
    fn capture_names_map_to_token_kinds() {
        assert_eq!(token_kind("keyword", "if"), Some(TokenKind::Keyword));
        assert_eq!(
            token_kind("string", "string_literal"),
            Some(TokenKind::String)
        );
        assert_eq!(
            token_kind("comment.documentation", "line_comment"),
            Some(TokenKind::Comment)
        );
        assert_eq!(
            token_kind("function.method", "identifier"),
            Some(TokenKind::Function)
        );
        assert_eq!(
            token_kind("type.builtin", "primitive_type"),
            Some(TokenKind::TypeName)
        );
        assert_eq!(
            token_kind("punctuation.bracket", "("),
            Some(TokenKind::Punctuation)
        );
        assert_eq!(
            token_kind("constant.builtin", "integer_literal"),
            Some(TokenKind::Number)
        );
        assert_eq!(
            token_kind("constant.builtin", "boolean_literal"),
            Some(TokenKind::Constant)
        );
        assert_eq!(token_kind("variable", "identifier"), None);
        assert_eq!(token_kind("property", "field_identifier"), None);
        assert_eq!(token_kind("embedded", "string_content"), None);
    }
}
