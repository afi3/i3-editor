//! Per-document structure: symbols for go-to-symbol and foldable line ranges.
//!
//! Uses the same tree-sitter grammars as highlighting, but parses on demand:
//! both commands are rare compared with rendering, and the highlighter tree is
//! owned by the render cache.

use std::ops::Range;

use ropey::Rope;
use tree_sitter::{Node, Parser, Tree};

use crate::syntax::Language;
use crate::syntax::tree::ts_language;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Zero-based line and byte column.
    pub line: usize,
    pub column: usize,
    pub name: String,
    pub kind: &'static str,
}

/// Definitions in source order. Nested definitions are kept, so an `impl`
/// block lists both itself and its methods.
pub fn symbols(rope: &Rope, language: Language) -> Vec<Symbol> {
    let Some(tree) = parse(rope, language) else {
        return Vec::new();
    };
    let text = rope.to_string();
    let mut out = Vec::new();
    walk_symbols(tree.root_node(), text.as_bytes(), language, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|symbol| seen.insert((symbol.line, symbol.column, symbol.name.clone())));
    out
}

/// Closed-fold candidates as inclusive line ranges.
pub fn folds(rope: &Rope, language: Language) -> Vec<Range<usize>> {
    let Some(tree) = parse(rope, language) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_folds(tree.root_node(), language, &mut out);
    out.sort_by_key(|range| (range.start, range.end));
    out.dedup();
    out
}

fn parse(rope: &Rope, language: Language) -> Option<Tree> {
    let grammar = ts_language(language)?;
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    parser.parse(rope.to_string(), None)
}

fn walk_symbols(node: Node<'_>, text: &[u8], language: Language, out: &mut Vec<Symbol>) {
    if let Some(kind) = symbol_kind(language, node.kind())
        && let Some(name) = node_label(node, text)
    {
        let start = node.start_position();
        out.push(Symbol {
            line: start.row,
            column: start.column,
            name,
            kind,
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_symbols(child, text, language, out);
    }
}

fn walk_folds(node: Node<'_>, language: Language, out: &mut Vec<Range<usize>>) {
    if foldable(language, node.kind()) {
        let start = node.start_position();
        let end = node.end_position();
        let mut last = end.row;
        if end.column == 0 && last > start.row {
            last -= 1;
        }
        if last > start.row {
            out.push(start.row..last);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_folds(child, language, out);
    }
}

fn node_label(node: Node<'_>, text: &[u8]) -> Option<String> {
    for field in ["name", "type", "key"] {
        if let Some(child) = node.child_by_field_name(field) {
            let bytes = text.get(child.byte_range())?;
            let label = String::from_utf8_lossy(bytes);
            let label = label.trim();
            if !label.is_empty() {
                return Some(label.chars().take(80).collect());
            }
        }
    }
    let start = node.start_byte();
    let end = node.end_byte().min(start.saturating_add(160));
    let snippet = std::str::from_utf8(text.get(start..end)?).ok()?;
    let line = snippet.lines().next()?.trim();
    let line = line.trim_start_matches('#').trim();
    if line.is_empty() {
        return None;
    }
    Some(line.chars().take(80).collect())
}

fn symbol_kind(language: Language, kind: &str) -> Option<&'static str> {
    match language {
        Language::Rust => rust_symbol(kind),
        Language::Python => python_symbol(kind),
        Language::JavaScript | Language::TypeScript => script_symbol(kind),
        Language::Go => go_symbol(kind),
        Language::C => c_symbol(kind),
        Language::Shell => shell_symbol(kind),
        Language::Lua => lua_symbol(kind),
        Language::Markdown => markdown_symbol(kind),
        Language::Toml => toml_symbol(kind),
        _ => None,
    }
}

fn rust_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_item" => "fn",
        "struct_item" => "struct",
        "enum_item" => "enum",
        "trait_item" => "trait",
        "impl_item" => "impl",
        "mod_item" => "mod",
        "const_item" => "const",
        "static_item" => "static",
        "type_item" => "type",
        "macro_definition" => "macro",
        _ => return None,
    })
}

fn python_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_definition" => "def",
        "class_definition" => "class",
        _ => return None,
    })
}

fn script_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_declaration" => "function",
        "class_declaration" => "class",
        "method_definition" => "method",
        "interface_declaration" => "interface",
        "type_alias_declaration" => "type",
        "enum_declaration" => "enum",
        _ => return None,
    })
}

fn go_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_declaration" => "func",
        "method_declaration" => "method",
        "type_declaration" => "type",
        _ => return None,
    })
}

fn c_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_definition" => "function",
        "struct_specifier" => "struct",
        "enum_specifier" => "enum",
        "union_specifier" => "union",
        "type_definition" => "typedef",
        _ => return None,
    })
}

fn shell_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_definition" => "function",
        _ => return None,
    })
}

fn lua_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_declaration" | "function_definition" => "function",
        _ => return None,
    })
}

fn markdown_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "atx_heading" | "setext_heading" => "heading",
        _ => return None,
    })
}

fn toml_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "table" | "table_array_element" => "table",
        _ => return None,
    })
}

fn foldable(language: Language, kind: &str) -> bool {
    match language {
        Language::Rust => matches!(
            kind,
            "function_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "impl_item"
                | "mod_item"
                | "match_expression"
                | "block"
        ),
        Language::Python => matches!(kind, "function_definition" | "class_definition"),
        Language::JavaScript | Language::TypeScript => matches!(
            kind,
            "function_declaration"
                | "class_declaration"
                | "method_definition"
                | "interface_declaration"
                | "arrow_function"
                | "object"
        ),
        Language::Go => matches!(
            kind,
            "function_declaration" | "method_declaration" | "type_declaration"
        ),
        Language::C => matches!(
            kind,
            "function_definition" | "struct_specifier" | "enum_specifier" | "union_specifier"
        ),
        Language::Shell => matches!(kind, "function_definition"),
        Language::Lua => matches!(kind, "function_declaration" | "function_definition"),
        Language::Markdown => matches!(kind, "section" | "fenced_code_block"),
        Language::Toml => matches!(kind, "table" | "table_array_element"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn rust_symbols_keep_source_order_and_nesting() {
        let text = "struct Beta {\n    n: u8,\n}\n\nimpl Beta {\n    fn alpha(&self) {}\n}\n";
        let symbols = symbols(&rope(text), Language::Rust);
        let labels: Vec<(&str, usize)> = symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line))
            .collect();
        assert!(labels.contains(&("Beta", 0)), "{labels:?}");
        assert!(labels.contains(&("Beta", 4)), "{labels:?}");
        assert!(labels.contains(&("alpha", 5)), "{labels:?}");
    }

    #[test]
    fn markdown_headings_become_symbols() {
        let text = "# Title\n\ntext\n\n## Part\n";
        let symbols = symbols(&rope(text), Language::Markdown);
        assert_eq!(symbols[0].name, "Title");
        assert_eq!(symbols[0].line, 0);
        assert_eq!(symbols[1].name, "Part");
        assert_eq!(symbols[1].line, 4);
    }

    #[test]
    fn folds_cover_a_function_body() {
        let text = "fn alpha() {\n    one();\n    two();\n}\n\nfn beta() {}\n";
        let folds = folds(&rope(text), Language::Rust);
        assert!(folds.contains(&(0..3)), "{folds:?}");
        assert!(!folds.iter().any(|range| range.contains(&5)), "{folds:?}");
    }

    #[test]
    fn markdown_sections_fold() {
        let text = "# One\na\nb\n\n# Two\nc\n";
        let folds = folds(&rope(text), Language::Markdown);
        assert!(folds.iter().any(|range| range.start == 0 && range.end >= 2));
    }

    #[test]
    fn plain_text_has_no_structure() {
        let rope = rope("one\ntwo\n");
        assert!(symbols(&rope, Language::Unknown).is_empty());
        assert!(folds(&rope, Language::Unknown).is_empty());
    }
}
