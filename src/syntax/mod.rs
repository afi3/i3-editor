//! Syntax highlighting tokens.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    TypeName,
    Constant,
    Operator,
    Punctuation,
    Text,
}

/// Char-offset span inside one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    C,
    Shell,
    Toml,
    Json,
    Markdown,
    Unknown,
}

impl Language {
    pub fn from_path(path: Option<&Path>) -> Language {
        let Some(extension) = path.and_then(|path| path.extension()) else {
            return Language::Unknown;
        };
        match extension.to_string_lossy().to_ascii_lowercase().as_str() {
            "rs" => Language::Rust,
            "py" | "pyi" => Language::Python,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "go" => Language::Go,
            "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" => Language::C,
            "sh" | "bash" | "zsh" => Language::Shell,
            "toml" => Language::Toml,
            "json" => Language::Json,
            "md" | "markdown" => Language::Markdown,
            _ => Language::Unknown,
        }
    }

    fn line_comment(self) -> Option<&'static str> {
        match self {
            Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Go
            | Language::C => Some("//"),
            Language::Python | Language::Shell | Language::Toml => Some("#"),
            _ => None,
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Language::Rust => RUST_KEYWORDS,
            Language::Python => PYTHON_KEYWORDS,
            Language::JavaScript | Language::TypeScript => JS_KEYWORDS,
            Language::Go => GO_KEYWORDS,
            Language::C => C_KEYWORDS,
            Language::Shell => SHELL_KEYWORDS,
            Language::Toml | Language::Json | Language::Markdown | Language::Unknown => &[],
        }
    }

    fn quoted(self, ch: char) -> bool {
        match self {
            Language::Rust => ch == '"',
            Language::Python => ch == '"' || ch == '\'',
            Language::JavaScript | Language::TypeScript => ch == '"' || ch == '\'' || ch == '`',
            Language::Go => ch == '"' || ch == '\'' || ch == '`',
            Language::C => ch == '"' || ch == '\'',
            Language::Shell => ch == '"' || ch == '\'',
            Language::Toml => ch == '"' || ch == '\'',
            Language::Json => ch == '"',
            Language::Markdown => ch == '`',
            Language::Unknown => false,
        }
    }

    fn block_comments(self) -> bool {
        matches!(
            self,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Go
                | Language::C
        )
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "yield",
];
const PYTHON_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while",
    "with", "yield", "self",
];
const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
];
const GO_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "false",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "nil",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "true",
    "type",
    "var",
];
const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
    "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register", "return",
    "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned",
    "void", "volatile", "while",
];
const SHELL_KEYWORDS: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in", "local",
    "return", "then", "until", "while", "export", "echo",
];

/// Highlight one line. Offsets are char indices relative to the line.
pub fn highlight_line(language: Language, line: &str) -> Vec<Span> {
    if language == Language::Unknown {
        return Vec::new();
    }
    if language == Language::Markdown {
        return markdown_line(line);
    }

    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];

        if let Some(marker) = language.line_comment() {
            let marker_chars: Vec<char> = marker.chars().collect();
            if starts_with(&chars, index, &marker_chars) {
                spans.push(Span {
                    start: index,
                    end: chars.len(),
                    kind: TokenKind::Comment,
                });
                break;
            }
        }
        if language.block_comments() && starts_with(&chars, index, &['/', '*']) {
            let mut end = chars.len();
            let mut cursor = index + 2;
            while cursor + 1 < chars.len() {
                if chars[cursor] == '*' && chars[cursor + 1] == '/' {
                    end = cursor + 2;
                    break;
                }
                cursor += 1;
            }
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::Comment,
            });
            index = end;
            continue;
        }
        if language.quoted(ch) {
            let end = string_end(&chars, index, ch);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::String,
            });
            index = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let end = number_end(&chars, index);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::Number,
            });
            index = end;
            continue;
        }
        if language == Language::Shell && ch == '$' {
            let end = identifier_end(&chars, index + 1).max(index + 1);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::Constant,
            });
            index = end;
            continue;
        }
        if is_identifier_start(ch) {
            let end = identifier_end(&chars, index);
            let word: String = chars[index..end].iter().collect();
            let kind = classify_word(language, &word, &chars, end);
            if kind != TokenKind::Text {
                spans.push(Span {
                    start: index,
                    end,
                    kind,
                });
            }
            index = end;
            continue;
        }
        if is_operator(ch) || is_punctuation(ch) {
            spans.push(Span {
                start: index,
                end: index + 1,
                kind: if is_punctuation(ch) {
                    TokenKind::Punctuation
                } else {
                    TokenKind::Operator
                },
            });
            index += 1;
            continue;
        }
        index += 1;
    }
    spans
}

fn classify_word(language: Language, word: &str, chars: &[char], end: usize) -> TokenKind {
    if language.keywords().contains(&word) {
        return TokenKind::Keyword;
    }
    let next = chars[end..].iter().find(|ch| !ch.is_whitespace());
    if matches!(next, Some('(')) {
        return TokenKind::Function;
    }
    if matches!(word, "true" | "false" | "null" | "None") {
        return TokenKind::Constant;
    }
    let mut uppercase = false;
    let mut lowercase = false;
    for ch in word.chars() {
        if ch.is_uppercase() {
            uppercase = true;
        } else if ch.is_lowercase() {
            lowercase = true;
        }
    }
    if uppercase && !lowercase {
        return TokenKind::Constant;
    }
    if word.chars().next().is_some_and(char::is_uppercase) {
        return TokenKind::TypeName;
    }
    TokenKind::Text
}

fn markdown_line(line: &str) -> Vec<Span> {
    let chars: Vec<char> = line.chars().collect();
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return vec![Span {
            start: 0,
            end: chars.len(),
            kind: TokenKind::Keyword,
        }];
    }
    if trimmed.starts_with('>') {
        return vec![Span {
            start: 0,
            end: chars.len(),
            kind: TokenKind::Comment,
        }];
    }
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '`' {
            let mut cursor = index + 1;
            while cursor < chars.len() && chars[cursor] != '`' {
                cursor += 1;
            }
            let end = (cursor + 1).min(chars.len());
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::String,
            });
            index = end;
        } else {
            index += 1;
        }
    }
    spans
}

fn starts_with(chars: &[char], index: usize, pattern: &[char]) -> bool {
    index + pattern.len() <= chars.len() && chars[index..index + pattern.len()] == *pattern
}

fn string_end(chars: &[char], start: usize, quote: char) -> usize {
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    chars.len()
}

fn number_end(chars: &[char], start: usize) -> usize {
    let mut index = start;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn identifier_end(chars: &[char], start: usize) -> usize {
    let mut index = start;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_alphanumeric() || ch == '_' {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn is_identifier_start(ch: char) -> bool {
    ch.is_alphabetic() || ch == '_'
}

fn is_operator(ch: char) -> bool {
    "+-*/%=<>!&|^~?".contains(ch)
}

fn is_punctuation(ch: char) -> bool {
    "(){}[];,.:".contains(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(language: Language, line: &str) -> Vec<(String, TokenKind)> {
        let chars: Vec<char> = line.chars().collect();
        highlight_line(language, line)
            .into_iter()
            .map(|span| {
                (
                    chars[span.start..span.end].iter().collect::<String>(),
                    span.kind,
                )
            })
            .collect()
    }

    #[test]
    fn detects_language_by_extension() {
        assert_eq!(Language::from_path(Some(Path::new("a.rs"))), Language::Rust);
        assert_eq!(
            Language::from_path(Some(Path::new("a.PY"))),
            Language::Python
        );
        assert_eq!(
            Language::from_path(Some(Path::new("a.ts"))),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Some(Path::new("a.toml"))),
            Language::Toml
        );
        assert_eq!(
            Language::from_path(Some(Path::new("a.xyz"))),
            Language::Unknown
        );
        assert_eq!(Language::from_path(None), Language::Unknown);
    }

    #[test]
    fn highlights_rust_keywords_types_numbers_comments() {
        let spans = kinds(Language::Rust, "let x: usize = 42; // note");
        assert_eq!(spans[0], ("let".to_string(), TokenKind::Keyword));
        assert!(spans.contains(&("42".to_string(), TokenKind::Number)));
        assert!(spans.contains(&("// note".to_string(), TokenKind::Comment)));
        assert!(spans.contains(&(";".to_string(), TokenKind::Punctuation)));
    }

    #[test]
    fn detects_functions_and_type_names() {
        let spans = kinds(Language::Rust, "fn main() -> Foo {}");
        assert_eq!(spans[0], ("fn".to_string(), TokenKind::Keyword));
        assert!(spans.contains(&("main".to_string(), TokenKind::Function)));
        assert!(spans.contains(&("Foo".to_string(), TokenKind::TypeName)));
    }

    #[test]
    fn strings_win_over_comment_markers() {
        let spans = kinds(Language::Rust, "let s = \"// not\"; // real");
        assert!(spans.contains(&("\"// not\"".to_string(), TokenKind::String)));
        assert!(spans.contains(&("// real".to_string(), TokenKind::Comment)));
    }

    #[test]
    fn escapes_do_not_end_a_string_early() {
        let spans = kinds(Language::Rust, "let s = \"a\\\"b\";");
        assert!(
            spans
                .iter()
                .any(|(text, kind)| text == "\"a\\\"b\"" && *kind == TokenKind::String),
            "{spans:?}"
        );
    }

    #[test]
    fn python_and_shell_use_hash_comments() {
        let python = kinds(Language::Python, "def f(x): return \"s\"  # c");
        assert!(python.contains(&("def".to_string(), TokenKind::Keyword)));
        assert!(python.contains(&("f".to_string(), TokenKind::Function)));
        assert!(python.contains(&("# c".to_string(), TokenKind::Comment)));

        let shell = kinds(Language::Shell, "echo $HOME # home");
        assert!(shell.contains(&("echo".to_string(), TokenKind::Keyword)));
        assert!(shell.contains(&("$HOME".to_string(), TokenKind::Constant)));
        assert!(shell.contains(&("# home".to_string(), TokenKind::Comment)));
    }

    #[test]
    fn markdown_headings_and_code_spans() {
        let heading = kinds(Language::Markdown, "## Title");
        assert_eq!(heading, vec![("## Title".to_string(), TokenKind::Keyword)]);
        let inline = kinds(Language::Markdown, "run `cargo test` now");
        assert!(inline.contains(&("`cargo test`".to_string(), TokenKind::String)));
    }

    #[test]
    fn unknown_language_produces_no_spans() {
        assert!(highlight_line(Language::Unknown, "let x = 1;").is_empty());
        assert!(highlight_line(Language::Toml, "").is_empty());
    }
}
