//! Search options and the matcher shared by Find, Replace, and Find in Files.

use std::ops::Range;

use regex::Regex;

/// Matches after which a search stops collecting; the UI stays bounded.
pub const MAX_MATCHES: usize = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub regex: bool,
    pub whole_word: bool,
    pub case_sensitive: bool,
}

impl Options {
    /// Short flags for prompt titles: `.*` regex, `W` whole word, `Aa` case.
    pub fn flags(self) -> String {
        let mut flags = Vec::new();
        if self.regex {
            flags.push(".*");
        }
        if self.whole_word {
            flags.push("W");
        }
        if self.case_sensitive {
            flags.push("Aa");
        }
        flags.join(" ")
    }

    pub fn summary(self) -> String {
        format!(
            "regex {}, whole word {}, case {}",
            if self.regex { "on" } else { "off" },
            if self.whole_word { "on" } else { "off" },
            if self.case_sensitive {
                "sensitive"
            } else {
                "insensitive"
            }
        )
    }
}

/// One compiled pattern. Literal mode escapes the query but reuses the same
/// regex engine, so offsets, whole-word boundaries, and replacement behaviour
/// stay identical across all modes.
pub struct Matcher {
    regex: Regex,
    literal: bool,
}

impl Matcher {
    pub fn new(query: &str, options: Options) -> Result<Self, String> {
        if query.is_empty() {
            return Err("empty search".to_string());
        }
        let mut source = if options.regex {
            query.to_string()
        } else {
            regex::escape(query)
        };
        if options.whole_word {
            source = format!(r"\b(?:{source})\b");
        }
        if !options.case_sensitive {
            source = format!("(?i){source}");
        }
        let regex = Regex::new(&source).map_err(|error| error.to_string())?;
        Ok(Self {
            regex,
            literal: !options.regex,
        })
    }

    /// Char offsets of every non-overlapping match, and whether more matches
    /// existed past `MAX_MATCHES`.
    pub fn find(&self, text: &str) -> (Vec<Range<usize>>, bool) {
        let mut out = Vec::new();
        let mut byte = 0usize;
        let mut chars = 0usize;
        let mut truncated = false;
        for found in self.regex.find_iter(text) {
            if found.start() == found.end() {
                continue;
            }
            if out.len() >= MAX_MATCHES {
                truncated = true;
                break;
            }
            chars += text[byte..found.start()].chars().count();
            let start = chars;
            chars += text[found.start()..found.end()].chars().count();
            byte = found.end();
            out.push(start..chars);
        }
        (out, truncated)
    }

    /// Replace every match. Literal replacements keep `$` literal.
    pub fn replace_all(&self, text: &str, replacement: &str) -> String {
        let replacement = if self.literal {
            replacement.replace('$', "$$")
        } else {
            replacement.to_string()
        };
        self.regex
            .replace_all(text, replacement.as_str())
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_reports_truncation_past_the_cap() {
        let matcher = Matcher::new("a", Options::default()).expect("matcher");
        let under = "a ".repeat(MAX_MATCHES);
        assert!(!matcher.find(&under).1);
        let over = "a ".repeat(MAX_MATCHES + 1);
        let (ranges, truncated) = matcher.find(&over);
        assert!(truncated);
        assert_eq!(ranges.len(), MAX_MATCHES);
    }

    #[test]
    fn literal_mode_is_case_insensitive_by_default() {
        let matcher = Matcher::new("needle", Options::default()).expect("matcher");
        assert_eq!(matcher.find("Needle needle").0, vec![0..6, 7..13]);
    }

    #[test]
    fn case_sensitive_and_whole_word_narrow_matches() {
        let options = Options {
            case_sensitive: true,
            whole_word: true,
            ..Options::default()
        };
        let matcher = Matcher::new("needle", options).expect("matcher");
        assert_eq!(matcher.find("needle needles NEEDLE").0, vec![0..6]);
    }

    #[test]
    fn regex_mode_supports_groups_and_offsets() {
        let matcher = Matcher::new(
            r"n(\w+)",
            Options {
                regex: true,
                ..Options::default()
            },
        )
        .expect("matcher");
        assert_eq!(matcher.find("no need").0, vec![0..2, 3..7]);
        assert_eq!(matcher.replace_all("no need", "<$1>"), "<o> <eed>");
    }

    #[test]
    fn invalid_regex_is_reported() {
        assert!(
            Matcher::new(
                "(",
                Options {
                    regex: true,
                    ..Options::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn literal_replacement_keeps_dollar_signs() {
        let matcher = Matcher::new("x", Options::default()).expect("matcher");
        assert_eq!(matcher.replace_all("x", "$1"), "$1");
    }

    #[test]
    fn unicode_offsets_are_chars() {
        let matcher = Matcher::new("好", Options::default()).expect("matcher");
        assert_eq!(matcher.find("你好！").0, vec![1..2]);
    }
}
