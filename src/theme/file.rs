//! Theme files: `<config>/i3-editor/themes/<name>.toml`.
//!
//! Every field is optional. `base` picks a preset to inherit from, so a file can
//! override only the colors it cares about. Unknown keys are rejected so a typo
//! is reported instead of silently ignored.

use std::path::Path;

use ratatui::style::Color;
use serde::Deserialize;

use super::{PRESETS, Theme};

macro_rules! set_colors {
    ($target:expr, $file:expr, $warnings:expr, $name:expr, [$( $field:ident ),* $(,)?]) => {
        $(
            if let Some(text) = &$file.$field {
                match parse_color(text) {
                    Ok(color) => $target.$field = color,
                    Err(error) => $warnings.push(format!(
                        "theme {}: {}: {error}",
                        $name,
                        stringify!($field)
                    )),
                }
            }
        )*
    };
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeFile {
    pub base: Option<String>,
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub current_line: Option<String>,
    pub selection: Option<String>,
    pub selection_text: Option<String>,
    pub gutter: Option<String>,
    pub gutter_active: Option<String>,
    pub border_active: Option<String>,
    pub border_inactive: Option<String>,
    pub tab_active: Option<String>,
    pub tab_inactive: Option<String>,
    pub status_bar: Option<String>,
    pub status_text: Option<String>,
    pub explorer_dir: Option<String>,
    pub prompt_border: Option<String>,
    pub syntax: SyntaxSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyntaxSection {
    pub keyword: Option<String>,
    pub string: Option<String>,
    pub comment: Option<String>,
    pub number: Option<String>,
    pub function: Option<String>,
    pub type_name: Option<String>,
    pub constant: Option<String>,
    pub operator: Option<String>,
    pub punctuation: Option<String>,
}

/// Theme names available as files in `dir` (file stems, sorted).
pub fn available(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

pub fn load(path: &Path, name: &str) -> (Theme, Vec<String>) {
    let mut warnings = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            warnings.push(format!(
                "theme {name}: cannot read {}: {error}",
                path.display()
            ));
            return (fallback(name), warnings);
        }
    };
    let parsed: ThemeFile = match toml::from_str(&text) {
        Ok(parsed) => parsed,
        Err(error) => {
            warnings.push(format!("theme {name}: {error}"));
            return (fallback(name), warnings);
        }
    };

    let mut theme = match parsed.base.as_deref() {
        Some(base)
            if PRESETS
                .iter()
                .any(|preset| preset.eq_ignore_ascii_case(base)) =>
        {
            Theme::preset(base)
        }
        Some(base) => {
            warnings.push(format!(
                "theme {name}: unknown base {base:?}; inheriting from default"
            ));
            Theme::default()
        }
        None => Theme::default(),
    };

    set_colors!(
        theme,
        parsed,
        warnings,
        name,
        [
            background,
            foreground,
            current_line,
            selection,
            selection_text,
            gutter,
            gutter_active,
            border_active,
            border_inactive,
            tab_active,
            tab_inactive,
            status_bar,
            status_text,
            explorer_dir,
            prompt_border
        ]
    );
    set_colors!(
        theme.syntax,
        parsed.syntax,
        warnings,
        name,
        [
            keyword,
            string,
            comment,
            number,
            function,
            type_name,
            constant,
            operator,
            punctuation
        ]
    );
    (theme, warnings)
}

fn fallback(name: &str) -> Theme {
    if PRESETS
        .iter()
        .any(|preset| preset.eq_ignore_ascii_case(name))
    {
        Theme::preset(name)
    } else {
        Theme::default()
    }
}

/// Parse `#rgb`, `#rrggbb`, `indexed:N`, or a color name.
pub fn parse_color(text: &str) -> Result<Color, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err("empty color".to_string());
    }
    if let Some(hex) = raw.strip_prefix('#') {
        let expanded = match hex.chars().count() {
            3 => hex.chars().flat_map(|ch| [ch, ch]).collect::<String>(),
            6 => hex.to_string(),
            _ => {
                return Err(format!("expected #rgb or #rrggbb, got {raw:?}"));
            }
        };
        let red = hex_pair(&expanded[0..2])?;
        let green = hex_pair(&expanded[2..4])?;
        let blue = hex_pair(&expanded[4..6])?;
        return Ok(Color::Rgb(red, green, blue));
    }
    if let Some(index) = raw.strip_prefix("indexed:") {
        return index
            .trim()
            .parse::<u8>()
            .map(Color::Indexed)
            .map_err(|_| format!("indexed value must be 0..=255, got {index:?}"));
    }
    match raw.to_ascii_lowercase().as_str() {
        "reset" | "default" => Ok(Color::Reset),
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "gray" | "grey" => Ok(Color::Gray),
        "darkgray" | "dark_gray" | "dark-grey" => Ok(Color::DarkGray),
        "white" => Ok(Color::White),
        other => Err(format!("unknown color name {other:?}")),
    }
}

fn hex_pair(text: &str) -> Result<u8, String> {
    u8::from_str_radix(text, 16).map_err(|_| format!("invalid hex {text:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "i3-theme-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn parses_hex_and_named_colors() {
        assert_eq!(parse_color("#1e1e1e"), Ok(Color::Rgb(30, 30, 30)));
        assert_eq!(parse_color("#f00"), Ok(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color("indexed:236"), Ok(Color::Indexed(236)));
        assert_eq!(parse_color(" Reset "), Ok(Color::Reset));
        assert_eq!(parse_color("cyan"), Ok(Color::Cyan));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("#gggggg").is_err());
        assert!(parse_color("indexed:999").is_err());
        assert!(parse_color("chartreuse").is_err());
        assert!(parse_color("").is_err());
    }

    #[test]
    fn loads_a_full_theme_file() {
        let dir = temp_dir();
        let path = dir.join("mine.toml");
        std::fs::write(
            &path,
            "background = \"#101010\"\nforeground = \"#e0e0e0\"\n\
             [syntax]\nkeyword = \"#ff0000\"\nstring = \"#00ff00\"\n",
        )
        .expect("write");
        let (theme, warnings) = load(&path, "mine");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme.background, Color::Rgb(16, 16, 16));
        assert_eq!(theme.syntax.keyword, Color::Rgb(255, 0, 0));
        assert_eq!(theme.syntax.string, Color::Rgb(0, 255, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_file_inherits_from_base() {
        let dir = temp_dir();
        let path = dir.join("based.toml");
        std::fs::write(&path, "base = \"dark\"\n[syntax]\nkeyword = \"#abcdef\"\n").expect("write");
        let (theme, warnings) = load(&path, "based");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme.background, Theme::preset("dark").background);
        assert_eq!(theme.syntax.keyword, Color::Rgb(171, 205, 239));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bad_color_warns_and_keeps_the_rest() {
        let dir = temp_dir();
        let path = dir.join("broken.toml");
        std::fs::write(&path, "background = \"#nope\"\nforeground = \"#ffffff\"\n").expect("write");
        let (theme, warnings) = load(&path, "broken");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("background"), "{warnings:?}");
        assert_eq!(theme.foreground, Color::Rgb(255, 255, 255));
        assert_eq!(theme.background, Theme::default().background);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_key_is_reported() {
        let dir = temp_dir();
        let path = dir.join("typo.toml");
        std::fs::write(&path, "background = \"#000000\"\nbackgroud = \"#ffffff\"\n")
            .expect("write");
        let (theme, warnings) = load(&path, "typo");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(theme, Theme::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn available_lists_file_stems() {
        let dir = temp_dir();
        std::fs::write(dir.join("a.toml"), "").expect("write");
        std::fs::write(dir.join("b.toml"), "").expect("write");
        std::fs::write(dir.join("notes.txt"), "").expect("write");
        assert_eq!(available(&dir), vec!["a".to_string(), "b".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_config_loads_a_theme_file_by_name() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("mine.toml"),
            "base = \"dark\"\n[syntax]\nkeyword = \"#123456\"\n",
        )
        .expect("write");
        let appearance = crate::config::AppearanceConfig {
            theme: "mine".to_string(),
            ..crate::config::AppearanceConfig::default()
        };
        let (theme, warnings) = Theme::from_config(&appearance, &dir);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme.syntax.keyword, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(theme.background, Theme::preset("dark").background);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_theme_file_overrides_a_preset_with_the_same_name() {
        let dir = temp_dir();
        std::fs::write(dir.join("dark.toml"), "background = \"#000000\"\n").expect("write");
        let appearance = crate::config::AppearanceConfig {
            theme: "dark".to_string(),
            ..crate::config::AppearanceConfig::default()
        };
        let (theme, warnings) = Theme::from_config(&appearance, &dir);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme.background, Color::Rgb(0, 0, 0));
        std::fs::remove_dir_all(&dir).ok();
    }
}
