//! Structural highlighting with tree-sitter.
//!
//! One parse tree is kept per document. Edits are applied to the tree as
//! [`InputEdit`]s, so the next frame reparses only the changed region. Spans are
//! produced by running the grammar's highlight query over the visible byte range
//! only.

use std::collections::HashMap;
use std::ops::Range;
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
    add(
        Language::Json,
        tree_sitter_json::LANGUAGE.into(),
        tree_sitter_json::HIGHLIGHTS_QUERY,
    );
    add(
        Language::Lua,
        tree_sitter_lua::LANGUAGE.into(),
        tree_sitter_lua::HIGHLIGHTS_QUERY,
    );
    add(
        Language::Toml,
        tree_sitter_toml_ng::LANGUAGE.into(),
        tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
    );
    add(
        Language::Markdown,
        tree_sitter_md::LANGUAGE.into(),
        tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
    );
    map
});

/// Inline Markdown (emphasis, links, code spans) is a second grammar.
static MARKDOWN_INLINE: LazyLock<Option<Grammar>> = LazyLock::new(|| {
    let language: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
    let query = Query::new(&language, tree_sitter_md::HIGHLIGHT_QUERY_INLINE).ok()?;
    Some(Grammar { language, query })
});

/// HTML injected into Markdown blocks and inline tags.
static MARKDOWN_HTML: LazyLock<Option<Grammar>> = LazyLock::new(|| {
    let language: tree_sitter::Language = tree_sitter_html::LANGUAGE.into();
    let query = Query::new(&language, tree_sitter_html::HIGHLIGHTS_QUERY).ok()?;
    Some(Grammar { language, query })
});

/// YAML front matter (`--- ... ---`) at the top of a Markdown document.
static MARKDOWN_YAML: LazyLock<Option<Grammar>> = LazyLock::new(|| {
    let language: tree_sitter::Language = tree_sitter_yaml::LANGUAGE.into();
    let query = Query::new(&language, tree_sitter_yaml::HIGHLIGHTS_QUERY).ok()?;
    Some(Grammar { language, query })
});

fn grammars() -> &'static HashMap<Language, Grammar> {
    &GRAMMARS
}

/// Whether `language` has a grammar and a compiled highlight query.
pub fn supports(language: Language) -> bool {
    grammars().contains_key(&language)
}

/// The tree-sitter grammar for `language`, when one is compiled in.
pub fn ts_language(language: Language) -> Option<tree_sitter::Language> {
    grammars()
        .get(&language)
        .map(|grammar| grammar.language.clone())
}

/// The parser state for one document.
#[derive(Default)]
pub(crate) struct Highlighter {
    parser: Option<Parser>,
    tree: Option<Tree>,
    language: Language,
    /// The tree was edited and must be handed back to the parser.
    dirty: bool,
    /// Markdown block content parsed again with the inline grammar and with
    /// each fenced block's language. Edits invalidate the whole cache because
    /// every sub-tree stores offsets from a different base.
    injected: Injected,
}

#[derive(Default)]
struct Injected {
    parser: Option<Parser>,
    inline: HashMap<usize, Tree>,
    fences: HashMap<usize, Tree>,
    html: HashMap<usize, Tree>,
    front_matter: HashMap<usize, Tree>,
}

impl Injected {
    /// Sub-trees are small, but a long document can expose many of them while
    /// scrolling. Drop the cache wholesale instead of tracking eviction order.
    const CACHE_LIMIT: usize = 256;

    fn clear(&mut self) {
        self.inline.clear();
        self.fences.clear();
        self.html.clear();
        self.front_matter.clear();
    }

    fn parse(&mut self, text: &str, language: &tree_sitter::Language) -> Option<Tree> {
        let parser = self.parser.get_or_insert_with(Parser::new);
        if parser.set_language(language).is_err() {
            return None;
        }
        parser.parse(text, None)
    }

    fn cache(cache: &mut HashMap<usize, Tree>, key: usize, tree: Tree) {
        if cache.len() >= Self::CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, tree);
    }
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
        self.injected.clear();
    }

    /// Keep the tree in sync with an edit that was just applied to the rope.
    pub fn apply_edit(&mut self, edit: tree_sitter::InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(&edit);
            self.dirty = true;
            self.injected.clear();
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

        let mut out = Output {
            rope,
            first,
            cells: &mut cells,
        };
        let mut cursor = QueryCursor::new();
        paint_query(
            &mut cursor,
            &grammar.query,
            tree,
            RopeText { rope },
            start_byte..end_byte,
            0,
            &mut out,
        );
        if language == Language::Markdown {
            markdown_spans(
                tree,
                &mut self.injected,
                start_byte,
                end_byte,
                &mut cursor,
                &mut out,
            );
        }

        Some(cells.into_iter().map(|cells| coalesce(&cells)).collect())
    }

    fn ensure_tree(&mut self, rope: &Rope, language: Language) -> bool {
        if self.language != language {
            self.language = language;
            self.tree = None;
            self.dirty = false;
            self.injected.clear();
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

/// Mutation target for painted captures: the rope, the first visible line, and
/// one cell buffer per visible line.
struct Output<'a> {
    rope: &'a Rope,
    first: usize,
    cells: &'a mut [Vec<Option<TokenKind>>],
}

/// Run one highlight query over `range` of a tree whose text starts at `offset`
/// bytes into the output rope. Markdown sub-trees are parsed from their own
/// slice, so their node offsets need the base added back.
fn paint_query<T, I>(
    cursor: &mut QueryCursor,
    query: &Query,
    tree: &Tree,
    text: T,
    range: Range<usize>,
    offset: usize,
    out: &mut Output<'_>,
) where
    T: tree_sitter::TextProvider<I>,
    I: AsRef<[u8]>,
{
    cursor.set_byte_range(range);
    let mut matches = cursor.matches(query, tree.root_node(), text);
    while let Some(found) = matches.next() {
        for capture in found.captures() {
            let Some(kind) = token_kind(
                query.capture_names()[capture.index as usize],
                capture.node.kind(),
            ) else {
                continue;
            };
            paint(
                out,
                offset + capture.node.start_byte(),
                offset + capture.node.end_byte(),
                kind,
            );
        }
    }
}

fn paint(out: &mut Output<'_>, start_byte: usize, end_byte: usize, kind: TokenKind) {
    let rope = out.rope;
    if start_byte >= end_byte || out.cells.is_empty() {
        return;
    }
    let first = out.first;
    let first_line = rope.byte_to_line(start_byte);
    let last_line = rope.byte_to_line(end_byte.saturating_sub(1));
    let last_visible = first + out.cells.len() - 1;
    for line in first_line..=last_line.min(last_visible) {
        if line < first {
            continue;
        }
        let line_byte = rope.line_to_byte(line);
        let line_start = rope.line_to_char(line);
        let len = visible_chars(rope, line);
        if len == 0 {
            continue;
        }
        let line_end = line_start + len;
        let start = rope.byte_to_char(start_byte.max(line_byte)) - line_start;
        let end = rope.byte_to_char(end_byte).min(line_end) - line_start;
        if start < end && start < len {
            out.cells[line - first][start..end.min(len)].fill(Some(kind));
        }
    }
}

/// Nodes in the visible Markdown block tree that need a second parser.
#[derive(Default)]
struct MarkdownParts {
    inline: Vec<Range<usize>>,
    fences: Vec<(Range<usize>, Language)>,
    html: Vec<Range<usize>>,
    yaml_metadata: Vec<Range<usize>>,
    toml_metadata: Vec<Range<usize>>,
}

/// Highlight the parts of Markdown that live in other trees: inline content
/// (emphasis, links, code spans), fenced code, HTML blocks/tags, and YAML or
/// TOML front matter.
fn markdown_spans(
    tree: &Tree,
    injected: &mut Injected,
    start_byte: usize,
    end_byte: usize,
    cursor: &mut QueryCursor,
    out: &mut Output<'_>,
) {
    let rope = out.rope;
    let mut parts = MarkdownParts::default();
    collect_markdown(tree.root_node(), rope, start_byte, end_byte, &mut parts);

    if let Some(inline) = MARKDOWN_INLINE.as_ref() {
        for range in parts.inline {
            paint_subtree(
                cursor,
                injected,
                SubKind::Inline,
                inline,
                range.clone(),
                out,
            );
            let tags: Vec<Range<usize>> = injected
                .inline
                .get(&range.start)
                .map(|sub| {
                    let mut tags = Vec::new();
                    collect_html_tags(sub.root_node(), &mut tags);
                    tags
                })
                .unwrap_or_default();
            if let Some(html) = MARKDOWN_HTML.as_ref() {
                for tag in tags {
                    let start = range.start + tag.start;
                    let end = range.start + tag.end;
                    paint_subtree(cursor, injected, SubKind::Html, html, start..end, out);
                }
            }
        }
    }

    for (range, language) in parts.fences {
        let Some(grammar) = grammars().get(&language) else {
            continue;
        };
        paint_subtree(cursor, injected, SubKind::Fence, grammar, range, out);
    }

    if let Some(html) = MARKDOWN_HTML.as_ref() {
        for range in parts.html {
            paint_subtree(cursor, injected, SubKind::Html, html, range, out);
        }
    }

    if let Some(yaml) = MARKDOWN_YAML.as_ref() {
        for range in parts.yaml_metadata {
            paint_subtree(cursor, injected, SubKind::FrontMatter, yaml, range, out);
        }
    }

    // The TOML parser reports the `+++` delimiters as error nodes; the pairs
    // between them still parse and highlight.
    if let Some(toml) = grammars().get(&Language::Toml) {
        for range in parts.toml_metadata {
            paint_subtree(cursor, injected, SubKind::FrontMatter, toml, range, out);
        }
    }
}

#[derive(Clone, Copy)]
enum SubKind {
    Inline,
    Fence,
    Html,
    FrontMatter,
}

impl Injected {
    fn cache_for(&mut self, kind: SubKind) -> &mut HashMap<usize, Tree> {
        match kind {
            SubKind::Inline => &mut self.inline,
            SubKind::Fence => &mut self.fences,
            SubKind::Html => &mut self.html,
            SubKind::FrontMatter => &mut self.front_matter,
        }
    }
}

fn paint_subtree(
    cursor: &mut QueryCursor,
    injected: &mut Injected,
    kind: SubKind,
    grammar: &Grammar,
    range: Range<usize>,
    out: &mut Output<'_>,
) {
    let text = rope_text(out.rope, range.clone());
    if !injected.cache_for(kind).contains_key(&range.start)
        && let Some(sub) = injected.parse(&text, &grammar.language)
    {
        Injected::cache(injected.cache_for(kind), range.start, sub);
    }
    if let Some(sub) = injected.cache_for(kind).get(&range.start) {
        paint_query(
            cursor,
            &grammar.query,
            sub,
            text.as_bytes(),
            0..text.len(),
            range.start,
            out,
        );
    }
}

fn collect_markdown(node: Node, rope: &Rope, start: usize, end: usize, parts: &mut MarkdownParts) {
    if node.end_byte() <= start || node.start_byte() >= end {
        return;
    }
    match node.kind() {
        "inline" => {
            parts.inline.push(node.byte_range());
            return;
        }
        "fenced_code_block" => {
            if let Some(fence) = fence(node, rope) {
                parts.fences.push(fence);
            }
            return;
        }
        "html_block" => {
            parts.html.push(node.byte_range());
            return;
        }
        "minus_metadata" => {
            parts.yaml_metadata.push(node.byte_range());
            return;
        }
        "plus_metadata" => {
            parts.toml_metadata.push(node.byte_range());
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_markdown(child, rope, start, end, parts);
    }
}

/// Tag ranges inside an inline Markdown tree, relative to that tree's text.
fn collect_html_tags(node: Node, out: &mut Vec<Range<usize>>) {
    if node.kind() == "html_tag" {
        out.push(node.byte_range());
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_html_tags(child, out);
    }
}

/// Language and content range of a fenced code block. Fences without a known
/// language, or nested Markdown, are left to the block query.
fn fence(node: Node, rope: &Rope) -> Option<(Range<usize>, Language)> {
    let mut content = None;
    let mut tag = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "code_fence_content" => content = Some(child.byte_range()),
            "info_string" => tag = Some(rope_text(rope, child.byte_range())),
            _ => {}
        }
    }
    let (content, tag) = (content?, tag?);
    let tag = tag
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
        .next()
        .unwrap_or("");
    let language = Language::from_tag(tag);
    (language != Language::Unknown && language != Language::Markdown).then_some((content, language))
}

fn rope_text(rope: &Rope, range: Range<usize>) -> String {
    rope.byte_slice(range).to_string()
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
    match capture {
        "text.title" => return Some(TokenKind::TypeName),
        // A fenced block is captured as a literal in the block query; its
        // content is highlighted by the embedded grammar instead.
        "text.literal" if node != "fenced_code_block" => return Some(TokenKind::String),
        "text.reference" => return Some(TokenKind::Function),
        "text.uri" => return Some(TokenKind::String),
        "boolean" => return Some(TokenKind::Constant),
        _ => {}
    }
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
        "tag" => TokenKind::Keyword,
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
            Language::Toml,
            Language::Json,
            Language::Lua,
            Language::Markdown,
        ] {
            assert!(supports(language), "{language:?}");
        }
        assert!(!supports(Language::Unknown));
        assert!(MARKDOWN_INLINE.is_some(), "inline markdown query");
        assert!(MARKDOWN_HTML.is_some(), "html markdown query");
        assert!(MARKDOWN_YAML.is_some(), "yaml front matter query");
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
