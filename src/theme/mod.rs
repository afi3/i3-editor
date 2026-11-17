//! Color themes: UI palette plus syntax token colors.
//!
//! `appearance.theme` names a built-in preset or a file in
//! `<config>/i3-editor/themes/<name>.toml`.

pub mod file;

use ratatui::style::{Color, Modifier, Style};

use crate::config::AppearanceConfig;
use crate::syntax::TokenKind;

pub const PRESETS: &[&str] = &["default", "dark", "light", "high-contrast"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxColors {
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    pub function: Color,
    pub type_name: Color,
    pub constant: Color,
    pub operator: Color,
    pub punctuation: Color,
}

impl Default for SyntaxColors {
    fn default() -> Self {
        Self {
            keyword: Color::Reset,
            string: Color::Reset,
            comment: Color::Reset,
            number: Color::Reset,
            function: Color::Reset,
            type_name: Color::Reset,
            constant: Color::Reset,
            operator: Color::Reset,
            punctuation: Color::Reset,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub current_line: Color,
    pub selection: Color,
    pub selection_text: Color,
    pub gutter: Color,
    pub gutter_active: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub status_bar: Color,
    pub status_text: Color,
    pub explorer_dir: Color,
    pub prompt_border: Color,
    pub syntax: SyntaxColors,
}

impl Default for Theme {
    fn default() -> Self {
        Self::preset("default")
    }
}

impl Theme {
    /// Resolve `appearance.theme`: a theme file wins over a preset with the same
    /// name, so adding `<name>.toml` overrides a built-in preset.
    pub fn from_config(
        appearance: &AppearanceConfig,
        themes_dir: &std::path::Path,
    ) -> (Self, Vec<String>) {
        let wanted = appearance.theme.trim();
        if wanted.is_empty() {
            return (Self::default(), Vec::new());
        }
        let path = themes_dir.join(format!("{wanted}.toml"));
        if path.is_file() {
            return file::load(&path, wanted);
        }
        if PRESETS
            .iter()
            .any(|preset| preset.eq_ignore_ascii_case(wanted))
        {
            return (Self::preset(wanted), Vec::new());
        }
        let available = file::available(themes_dir);
        let mut names = PRESETS
            .iter()
            .map(|preset| (*preset).to_string())
            .collect::<Vec<_>>();
        names.extend(available);
        (
            Self::default(),
            vec![format!(
                "unknown theme {wanted:?}; using default (available: {})",
                names.join(", ")
            )],
        )
    }

    /// All selectable theme names: presets first, then theme files.
    pub fn names(themes_dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = PRESETS.iter().map(|preset| (*preset).to_string()).collect();
        for name in file::available(themes_dir) {
            if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                names.push(name);
            }
        }
        names
    }

    /// Preset name this theme matches, for diagnostics.
    pub fn name(&self) -> &'static str {
        PRESETS
            .iter()
            .copied()
            .find(|preset| Theme::preset(preset) == *self)
            .unwrap_or("custom")
    }

    pub fn token_color(&self, kind: TokenKind) -> Color {
        match kind {
            TokenKind::Keyword => self.syntax.keyword,
            TokenKind::String => self.syntax.string,
            TokenKind::Comment => self.syntax.comment,
            TokenKind::Number => self.syntax.number,
            TokenKind::Function => self.syntax.function,
            TokenKind::TypeName => self.syntax.type_name,
            TokenKind::Constant => self.syntax.constant,
            TokenKind::Operator => self.syntax.operator,
            TokenKind::Punctuation => self.syntax.punctuation,
            TokenKind::Text => self.foreground,
        }
    }

    pub fn base(&self) -> Style {
        Style::default().bg(self.background).fg(self.foreground)
    }

    pub fn current_line(&self) -> Style {
        Style::default().bg(self.current_line).fg(self.foreground)
    }

    pub fn selection_style(&self) -> Style {
        Style::default().bg(self.selection).fg(self.selection_text)
    }

    pub fn tab_active_style(&self) -> Style {
        Style::default()
            .bg(self.tab_active)
            .fg(self.foreground)
            .add_modifier(Modifier::BOLD)
    }

    pub fn tab_inactive_style(&self) -> Style {
        Style::default().bg(self.background).fg(self.tab_inactive)
    }

    pub fn status_style(&self) -> Style {
        Style::default().bg(self.status_bar).fg(self.status_text)
    }

    pub fn preset(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "dark" => Self {
                background: Color::Indexed(234),
                foreground: Color::Indexed(252),
                current_line: Color::Indexed(236),
                selection: Color::Indexed(24),
                selection_text: Color::Indexed(255),
                gutter: Color::Indexed(243),
                gutter_active: Color::Indexed(252),
                border_active: Color::Indexed(45),
                border_inactive: Color::Indexed(240),
                tab_active: Color::Indexed(238),
                tab_inactive: Color::Indexed(240),
                status_bar: Color::Indexed(236),
                status_text: Color::Indexed(252),
                explorer_dir: Color::Indexed(75),
                prompt_border: Color::Indexed(45),
                syntax: SyntaxColors {
                    keyword: Color::Rgb(86, 156, 214),
                    string: Color::Rgb(206, 145, 120),
                    comment: Color::Rgb(106, 153, 85),
                    number: Color::Rgb(181, 206, 168),
                    function: Color::Rgb(220, 220, 170),
                    type_name: Color::Rgb(78, 201, 176),
                    constant: Color::Rgb(79, 193, 255),
                    operator: Color::Rgb(212, 212, 212),
                    punctuation: Color::Indexed(243),
                },
            },
            "light" => Self {
                background: Color::Indexed(255),
                foreground: Color::Indexed(235),
                current_line: Color::Indexed(254),
                selection: Color::Indexed(153),
                selection_text: Color::Indexed(232),
                gutter: Color::Indexed(245),
                gutter_active: Color::Indexed(235),
                border_active: Color::Indexed(25),
                border_inactive: Color::Indexed(250),
                tab_active: Color::Indexed(252),
                tab_inactive: Color::Indexed(246),
                status_bar: Color::Indexed(253),
                status_text: Color::Indexed(235),
                explorer_dir: Color::Indexed(25),
                prompt_border: Color::Indexed(25),
                syntax: SyntaxColors {
                    keyword: Color::Rgb(0, 0, 255),
                    string: Color::Rgb(163, 21, 21),
                    comment: Color::Rgb(0, 128, 0),
                    number: Color::Rgb(9, 134, 88),
                    function: Color::Rgb(121, 94, 38),
                    type_name: Color::Rgb(38, 127, 153),
                    constant: Color::Rgb(0, 112, 193),
                    operator: Color::Indexed(235),
                    punctuation: Color::Indexed(242),
                },
            },
            "high-contrast" => Self {
                background: Color::Black,
                foreground: Color::White,
                current_line: Color::Indexed(238),
                selection: Color::Yellow,
                selection_text: Color::Black,
                gutter: Color::White,
                gutter_active: Color::Yellow,
                border_active: Color::Yellow,
                border_inactive: Color::White,
                tab_active: Color::Yellow,
                tab_inactive: Color::Indexed(240),
                status_bar: Color::White,
                status_text: Color::Black,
                explorer_dir: Color::Yellow,
                prompt_border: Color::Yellow,
                syntax: SyntaxColors {
                    keyword: Color::Yellow,
                    string: Color::LightGreen,
                    comment: Color::LightCyan,
                    number: Color::LightMagenta,
                    function: Color::LightYellow,
                    type_name: Color::LightBlue,
                    constant: Color::LightRed,
                    operator: Color::White,
                    punctuation: Color::White,
                },
            },
            _ => Self {
                background: Color::Reset,
                foreground: Color::Reset,
                current_line: Color::Reset,
                selection: Color::Blue,
                selection_text: Color::Reset,
                gutter: Color::DarkGray,
                gutter_active: Color::Reset,
                border_active: Color::Cyan,
                border_inactive: Color::DarkGray,
                tab_active: Color::Reset,
                tab_inactive: Color::DarkGray,
                status_bar: Color::Reset,
                status_text: Color::Reset,
                explorer_dir: Color::Cyan,
                prompt_border: Color::Cyan,
                syntax: SyntaxColors {
                    keyword: Color::Blue,
                    string: Color::Green,
                    comment: Color::DarkGray,
                    number: Color::Magenta,
                    function: Color::Yellow,
                    type_name: Color::Cyan,
                    constant: Color::Magenta,
                    operator: Color::Reset,
                    punctuation: Color::Reset,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn appearance(name: &str) -> AppearanceConfig {
        AppearanceConfig {
            theme: name.to_string(),
            ..AppearanceConfig::default()
        }
    }

    fn nowhere() -> &'static Path {
        Path::new("/nonexistent-i3-editor-themes")
    }

    #[test]
    fn every_preset_resolves_without_warnings() {
        for preset in PRESETS {
            let (theme, warnings) = Theme::from_config(&appearance(preset), nowhere());
            assert!(warnings.is_empty(), "{preset}: {warnings:?}");
            assert_eq!(theme, Theme::preset(preset));
        }
    }

    #[test]
    fn unknown_theme_falls_back_and_warns() {
        let (theme, warnings) = Theme::from_config(&appearance("neon"), nowhere());
        assert_eq!(theme, Theme::preset("default"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("neon"), "{warnings:?}");
        assert!(warnings[0].contains("dark"), "{warnings:?}");
    }

    #[test]
    fn empty_theme_is_default_without_noise() {
        let (theme, warnings) = Theme::from_config(&appearance(""), nowhere());
        assert!(warnings.is_empty());
        assert_eq!(theme, Theme::preset("default"));
    }

    #[test]
    fn names_are_case_insensitive() {
        let (theme, _) = Theme::from_config(&appearance("DARK"), nowhere());
        assert_eq!(theme, Theme::preset("dark"));
    }

    #[test]
    fn presets_have_distinct_syntax_colors() {
        for preset in ["default", "dark", "light", "high-contrast"] {
            let theme = Theme::preset(preset);
            assert_ne!(theme.syntax.keyword, theme.foreground, "{preset}");
            assert_ne!(theme.syntax.string, theme.syntax.keyword, "{preset}");
            assert_ne!(theme.syntax.comment, theme.foreground, "{preset}");
        }
    }

    #[test]
    fn token_colors_follow_the_palette() {
        let theme = Theme::preset("dark");
        assert_eq!(theme.token_color(TokenKind::Keyword), theme.syntax.keyword);
        assert_eq!(theme.token_color(TokenKind::String), theme.syntax.string);
        assert_eq!(theme.token_color(TokenKind::Text), theme.foreground);
    }
}
