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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// Lexer state at the start of a line. Highlighting one line consumes this and
/// returns the state for the next line, so constructs that cross line breaks keep
/// their token kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LexState {
    #[default]
    Normal,
    /// Inside `/* ... */`; `depth` counts nested Rust comments.
    BlockComment { depth: usize },
    /// Inside a string or template literal that may span lines.
    String(Quote),
    /// Inside a Markdown fenced block; `marker` and `len` identify the closing run.
    Fence {
        marker: char,
        len: usize,
        language: Language,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quote {
    /// Python `"""`.
    Double,
    /// Python `'''`.
    Single,
    /// JavaScript template literal or Go raw string.
    Backtick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    #[default]
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

    /// Language named by a Markdown fence info string.
    pub fn from_tag(tag: &str) -> Language {
        match tag.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Language::Rust,
            "python" | "py" => Language::Python,
            "javascript" | "js" | "jsx" => Language::JavaScript,
            "typescript" | "ts" | "tsx" => Language::TypeScript,
            "go" => Language::Go,
            "c" | "h" | "cpp" => Language::C,
            "shell" | "sh" | "bash" | "zsh" => Language::Shell,
            "toml" => Language::Toml,
            "json" => Language::Json,
            "markdown" | "md" => Language::Markdown,
            _ => Language::Unknown,
        }
    }

    /// Rust nests `/* /* */ */`; the other C-like languages do not.
    fn nests_comments(self) -> bool {
        self == Language::Rust
    }

    fn multiline_strings(self) -> bool {
        matches!(
            self,
            Language::Python | Language::JavaScript | Language::TypeScript | Language::Go
        )
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

/// Highlight one line, consuming and returning the cross-line lexer state.
/// Offsets are char indices relative to the line.
pub fn line_spans(language: Language, line: &str, state: LexState) -> (Vec<Span>, LexState) {
    if language == Language::Unknown {
        return (Vec::new(), LexState::Normal);
    }
    let chars: Vec<char> = line.chars().collect();
    if language == Language::Markdown {
        return markdown_line(&chars, state);
    }
    let state = match state {
        LexState::BlockComment { .. } if !language.block_comments() => LexState::Normal,
        LexState::String(_) if !language.multiline_strings() => LexState::Normal,
        other => other,
    };

    match state {
        LexState::BlockComment { depth } => {
            let (end, remaining) = scan_block_comment(&chars, 0, language.nests_comments(), depth);
            if remaining > 0 {
                let span = Span {
                    start: 0,
                    end: chars.len(),
                    kind: TokenKind::Comment,
                };
                return (vec![span], LexState::BlockComment { depth: remaining });
            }
            let mut spans = Vec::new();
            if end > 0 {
                spans.push(Span {
                    start: 0,
                    end,
                    kind: TokenKind::Comment,
                });
            }
            let (rest, next) = scan_normal(language, &chars, end);
            spans.extend(rest);
            (spans, next)
        }
        LexState::String(quote) => {
            let escapes = language != Language::Go;
            let (end, closed) = scan_multiline_string(&chars, 0, quote, escapes);
            let mut spans = Vec::new();
            if end > 0 {
                spans.push(Span {
                    start: 0,
                    end,
                    kind: TokenKind::String,
                });
            }
            if !closed {
                return (spans, LexState::String(quote));
            }
            let (rest, next) = scan_normal(language, &chars, end);
            spans.extend(rest);
            (spans, next)
        }
        _ => scan_normal(language, &chars, 0),
    }
}

/// Lex a line from `start`, returning spans and the state left for the next line.
fn scan_normal(language: Language, chars: &[char], start: usize) -> (Vec<Span>, LexState) {
    let mut spans = Vec::new();
    let mut index = start;
    while index < chars.len() {
        let ch = chars[index];

        if let Some(marker) = language.line_comment() {
            let marker_chars: Vec<char> = marker.chars().collect();
            if starts_with(chars, index, &marker_chars) {
                spans.push(Span {
                    start: index,
                    end: chars.len(),
                    kind: TokenKind::Comment,
                });
                break;
            }
        }
        if language.block_comments() && starts_with(chars, index, &['/', '*']) {
            let (end, depth) = scan_block_comment(chars, index + 2, language.nests_comments(), 1);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::Comment,
            });
            if depth > 0 {
                return (spans, LexState::BlockComment { depth });
            }
            index = end;
            continue;
        }
        if language == Language::Python
            && matches!(ch, '"' | '\'')
            && starts_with(chars, index, &[ch, ch, ch])
        {
            let quote = if ch == '"' {
                Quote::Double
            } else {
                Quote::Single
            };
            let (end, closed) = scan_multiline_string(chars, index + 3, quote, true);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::String,
            });
            if !closed {
                return (spans, LexState::String(quote));
            }
            index = end;
            continue;
        }
        if ch == '`'
            && matches!(
                language,
                Language::JavaScript | Language::TypeScript | Language::Go
            )
        {
            let escapes = language != Language::Go;
            let (end, closed) = scan_multiline_string(chars, index + 1, Quote::Backtick, escapes);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::String,
            });
            if !closed {
                return (spans, LexState::String(Quote::Backtick));
            }
            index = end;
            continue;
        }
        if language.quoted(ch) {
            let end = string_end(chars, index, ch);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::String,
            });
            index = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let end = number_end(chars, index);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::Number,
            });
            index = end;
            continue;
        }
        if language == Language::Shell && ch == '$' {
            let end = identifier_end(chars, index + 1).max(index + 1);
            spans.push(Span {
                start: index,
                end,
                kind: TokenKind::Constant,
            });
            index = end;
            continue;
        }
        if is_identifier_start(ch) {
            let end = identifier_end(chars, index);
            let word: String = chars[index..end].iter().collect();
            let kind = classify_word(language, &word, chars, end);
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
    (spans, LexState::Normal)
}

/// Scan a block comment body starting after `/*` (or at the line start when the
/// comment continues). Returns the index after the closing `*/`, or the line end,
/// plus the depth still open.
fn scan_block_comment(
    chars: &[char],
    from: usize,
    nested: bool,
    mut depth: usize,
) -> (usize, usize) {
    let mut index = from;
    while index < chars.len() {
        if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return (index, 0);
            }
            continue;
        }
        if nested && chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
            depth += 1;
            index += 2;
            continue;
        }
        index += 1;
    }
    (chars.len(), depth)
}

/// Scan for the closing delimiter of a multiline string. Returns the index after
/// the delimiter and whether it was found.
fn scan_multiline_string(
    chars: &[char],
    from: usize,
    quote: Quote,
    escapes: bool,
) -> (usize, bool) {
    let terminator: &[char] = match quote {
        Quote::Double => &['"', '"', '"'],
        Quote::Single => &['\'', '\'', '\''],
        Quote::Backtick => &['`'],
    };
    let mut index = from;
    while index < chars.len() {
        if escapes && chars[index] == '\\' {
            index += 2;
            continue;
        }
        if starts_with(chars, index, terminator) {
            return (index + terminator.len(), true);
        }
        index += 1;
    }
    (chars.len(), false)
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

fn markdown_line(chars: &[char], state: LexState) -> (Vec<Span>, LexState) {
    if let LexState::Fence {
        marker,
        len,
        language,
    } = state
    {
        if fence_closes(chars, marker, len) {
            return (whole_line(chars, TokenKind::Punctuation), LexState::Normal);
        }
        let spans = if matches!(language, Language::Unknown | Language::Markdown) {
            Vec::new()
        } else {
            // Fence bodies are highlighted per line: a fenced snippet is not a
            // compiled unit, so a block comment inside it does not carry over.
            scan_normal(language, chars, 0).0
        };
        return (spans, state);
    }
    if let Some((marker, len, language)) = fence_opens(chars) {
        return (
            whole_line(chars, TokenKind::Punctuation),
            LexState::Fence {
                marker,
                len,
                language,
            },
        );
    }
    (markdown_plain(chars), LexState::Normal)
}

fn whole_line(chars: &[char], kind: TokenKind) -> Vec<Span> {
    if chars.is_empty() {
        Vec::new()
    } else {
        vec![Span {
            start: 0,
            end: chars.len(),
            kind,
        }]
    }
}

/// Opening fence: up to three leading spaces, then at least three backticks or
/// tildes, then an optional language tag.
fn fence_opens(chars: &[char]) -> Option<(char, usize, Language)> {
    let start = fence_indent(chars)?;
    let marker = *chars.get(start)?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let len = chars[start..]
        .iter()
        .take_while(|ch| **ch == marker)
        .count();
    if len < 3 {
        return None;
    }
    let info: String = chars[start + len..].iter().collect();
    let tag = info.split_whitespace().next().unwrap_or("");
    Some((marker, len, Language::from_tag(tag)))
}

/// Closing fence: at least as long as the opening run, nothing but whitespace after.
fn fence_closes(chars: &[char], marker: char, len: usize) -> bool {
    let Some(start) = fence_indent(chars) else {
        return false;
    };
    let run = chars[start..]
        .iter()
        .take_while(|ch| **ch == marker)
        .count();
    run >= len && chars[start + run..].iter().all(|ch| ch.is_whitespace())
}

fn fence_indent(chars: &[char]) -> Option<usize> {
    let mut index = 0;
    while index < 3 && chars.get(index) == Some(&' ') {
        index += 1;
    }
    (index < chars.len()).then_some(index)
}

fn markdown_plain(chars: &[char]) -> Vec<Span> {
    let start = chars
        .iter()
        .position(|ch| !ch.is_whitespace())
        .unwrap_or(chars.len());
    match chars.get(start) {
        Some('#') => return whole_line(chars, TokenKind::Keyword),
        Some('>') => return whole_line(chars, TokenKind::Comment),
        _ => {}
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
        line_spans(language, line, LexState::Normal)
            .0
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
        assert!(
            line_spans(Language::Unknown, "let x = 1;", LexState::Normal)
                .0
                .is_empty()
        );
        assert!(
            line_spans(Language::Toml, "", LexState::Normal)
                .0
                .is_empty()
        );
    }

    fn spans_from(
        language: Language,
        line: &str,
        state: LexState,
    ) -> (Vec<(String, TokenKind)>, LexState) {
        let chars: Vec<char> = line.chars().collect();
        let (spans, next) = line_spans(language, line, state);
        let out = spans
            .into_iter()
            .map(|span| {
                (
                    chars[span.start..span.end].iter().collect::<String>(),
                    span.kind,
                )
            })
            .collect();
        (out, next)
    }

    #[test]
    fn block_comments_carry_across_lines() {
        let (spans, state) = spans_from(Language::Rust, "let x = 1; /* open", LexState::Normal);
        assert_eq!(
            spans.last(),
            Some(&("/* open".to_string(), TokenKind::Comment))
        );
        assert_eq!(state, LexState::BlockComment { depth: 1 });

        let (spans, state) = spans_from(Language::Rust, "still inside", state);
        assert_eq!(
            spans,
            vec![("still inside".to_string(), TokenKind::Comment)]
        );
        assert_eq!(state, LexState::BlockComment { depth: 1 });

        let (spans, state) = spans_from(Language::Rust, "close */ let y = 2;", state);
        assert_eq!(state, LexState::Normal);
        assert_eq!(spans[0], ("close */".to_string(), TokenKind::Comment));
        assert!(spans.contains(&("let".to_string(), TokenKind::Keyword)));
        assert!(spans.contains(&("2".to_string(), TokenKind::Number)));
    }

    #[test]
    fn rust_block_comments_nest_but_c_ones_do_not() {
        let line = "/* a /* b */ c */ let z";
        let (spans, state) = spans_from(Language::Rust, line, LexState::Normal);
        assert_eq!(
            spans[0],
            ("/* a /* b */ c */".to_string(), TokenKind::Comment)
        );
        assert!(spans.contains(&("let".to_string(), TokenKind::Keyword)));
        assert_eq!(state, LexState::Normal);

        let (spans, _) = spans_from(Language::C, line, LexState::Normal);
        assert_eq!(spans[0], ("/* a /* b */".to_string(), TokenKind::Comment));
    }

    #[test]
    fn python_triple_quoted_strings_span_lines() {
        let (spans, state) = spans_from(Language::Python, "x = \"\"\"start", LexState::Normal);
        assert!(spans.contains(&("\"\"\"start".to_string(), TokenKind::String)));
        assert_eq!(state, LexState::String(Quote::Double));

        let (spans, state) = spans_from(Language::Python, "middle", state);
        assert_eq!(spans, vec![("middle".to_string(), TokenKind::String)]);

        let (spans, state) = spans_from(Language::Python, "end\"\"\" + y", state);
        assert_eq!(spans[0], ("end\"\"\"".to_string(), TokenKind::String));
        assert!(spans.contains(&("+".to_string(), TokenKind::Operator)));
        assert_eq!(state, LexState::Normal);
    }

    #[test]
    fn javascript_templates_and_go_raw_strings_span_lines() {
        let (spans, state) = spans_from(Language::JavaScript, "const s = `hello", LexState::Normal);
        assert!(spans.contains(&("`hello".to_string(), TokenKind::String)));
        assert_eq!(state, LexState::String(Quote::Backtick));

        let (spans, state) = spans_from(Language::JavaScript, "end`; let x", state);
        assert_eq!(state, LexState::Normal);
        assert_eq!(spans[0], ("end`".to_string(), TokenKind::String));
        assert!(spans.contains(&("let".to_string(), TokenKind::Keyword)));

        let (_, state) = spans_from(Language::Go, "s := `raw", LexState::Normal);
        assert_eq!(state, LexState::String(Quote::Backtick));
        let (spans, state) = spans_from(Language::Go, "done`", state);
        assert_eq!(spans, vec![("done`".to_string(), TokenKind::String)]);
        assert_eq!(state, LexState::Normal);
    }

    #[test]
    fn markdown_fences_highlight_their_body() {
        let (spans, state) = spans_from(Language::Markdown, "```rust", LexState::Normal);
        assert_eq!(spans, vec![("```rust".to_string(), TokenKind::Punctuation)]);
        assert_eq!(
            state,
            LexState::Fence {
                marker: '`',
                len: 3,
                language: Language::Rust
            }
        );

        let (spans, next) = spans_from(Language::Markdown, "fn main() {", state);
        assert_eq!(next, state);
        assert!(spans.contains(&("fn".to_string(), TokenKind::Keyword)));
        assert!(spans.contains(&("main".to_string(), TokenKind::Function)));

        let (spans, next) = spans_from(Language::Markdown, "```", state);
        assert_eq!(spans, vec![("```".to_string(), TokenKind::Punctuation)]);
        assert_eq!(next, LexState::Normal);
    }

    #[test]
    fn tilde_fences_and_unknown_states_are_handled() {
        let (_, state) = spans_from(Language::Markdown, "  ~~~~python", LexState::Normal);
        assert_eq!(
            state,
            LexState::Fence {
                marker: '~',
                len: 4,
                language: Language::Python
            }
        );
        let (_, next) = spans_from(Language::Markdown, "~~~~", state);
        assert_eq!(next, LexState::Normal);

        let (spans, state) =
            spans_from(Language::Python, "let", LexState::BlockComment { depth: 1 });
        assert!(spans.is_empty());
        assert_eq!(state, LexState::Normal);
    }
}
