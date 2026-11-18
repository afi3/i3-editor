//! Configuration: TOML at `~/.config/i3-editor/config.toml`, with defaults for
//! every missing section.
//!
//! A broken section is replaced by its default and reported, so a typo never
//! prevents the editor from starting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub editor: EditorConfig,
    pub explorer: ExplorerConfig,
    pub workspace: WorkspaceConfig,
    pub appearance: AppearanceConfig,
    pub terminal: TerminalConfig,
    pub keys: KeysConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Height of the bottom terminal panel in rows.
    pub height: u16,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self { height: 12 }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct EditorConfig {
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub line_numbers: bool,
    pub word_wrap: bool,
    pub auto_indent: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            line_numbers: true,
            word_wrap: false,
            auto_indent: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ExplorerConfig {
    pub visible: bool,
    /// Percentage of the terminal width, clamped to `5..=50`.
    pub width: u16,
    pub show_hidden: bool,
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self {
            visible: true,
            width: 20,
            show_hidden: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub default_layout: String,
    pub auto_save_session: bool,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            default_layout: "vertical".to_string(),
            auto_save_session: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: String,
    pub show_status_bar: bool,
    pub show_document_tabs: bool,
    pub show_workspace_tabs: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            show_status_bar: true,
            show_document_tabs: true,
            show_workspace_tabs: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct KeysConfig {
    pub preset: Option<String>,
    pub global: BTreeMap<String, String>,
    pub editing: BTreeMap<String, String>,
    pub explorer: BTreeMap<String, String>,
}

pub fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("i3-editor"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("i3-editor"))
}

pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("themes"))
}

pub fn default_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.toml"))
}

pub fn load() -> (AppConfig, Vec<String>) {
    match default_path() {
        Some(path) => load_from(&path),
        None => (AppConfig::default(), Vec::new()),
    }
}

pub fn load_from(path: &Path) -> (AppConfig, Vec<String>) {
    let mut warnings = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (AppConfig::default(), warnings);
        }
        Err(error) => {
            warnings.push(format!("cannot read {}: {error}", path.display()));
            return (AppConfig::default(), warnings);
        }
    };

    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("{} is not valid TOML: {error}", path.display()));
            return (AppConfig::default(), warnings);
        }
    };

    let mut config = AppConfig::default();
    take_section(&value, "editor", &mut config.editor, &mut warnings);
    take_section(&value, "explorer", &mut config.explorer, &mut warnings);
    take_section(&value, "workspace", &mut config.workspace, &mut warnings);
    take_section(&value, "appearance", &mut config.appearance, &mut warnings);
    take_section(&value, "terminal", &mut config.terminal, &mut warnings);
    take_section(&value, "keys", &mut config.keys, &mut warnings);
    config.explorer.width = config.explorer.width.clamp(5, 50);
    config.terminal.height = config.terminal.height.clamp(3, 60);
    (config, warnings)
}

fn take_section<T: for<'de> Deserialize<'de> + std::fmt::Debug>(
    value: &toml::Value,
    name: &str,
    target: &mut T,
    warnings: &mut Vec<String>,
) {
    let Some(section) = value.get(name) else {
        return;
    };
    match section.clone().try_into::<T>() {
        Ok(parsed) => *target = parsed,
        Err(error) => warnings.push(format!("[{name}] ignored: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults_without_warnings() {
        let (config, warnings) = load_from(Path::new("/nonexistent/i3-editor/config.toml"));
        assert!(warnings.is_empty());
        assert_eq!(config.editor.tab_size, 4);
        assert_eq!(config.explorer.width, 20);
    }

    #[test]
    fn defaults_fill_missing_sections() {
        let dir = std::env::temp_dir().join(format!("i3-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_size = 2\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert!(warnings.is_empty());
        assert_eq!(config.editor.tab_size, 2);
        assert!(config.editor.auto_indent);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_section_is_reported_and_replaced() {
        let dir = std::env::temp_dir().join(format!("i3-config-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\ntab_size = \"four\"\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(config.editor.tab_size, 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_toml_is_reported_and_ignored() {
        let dir = std::env::temp_dir().join(format!("i3-config-syntax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor\n").expect("write");
        let (config, warnings) = load_from(&path);
        assert_eq!(warnings.len(), 1);
        assert_eq!(config.editor.tab_size, 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explorer_width_is_clamped() {
        let dir = std::env::temp_dir().join(format!("i3-config-width-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[explorer]\nwidth = 90\n").expect("write");
        let (config, _) = load_from(&path);
        assert_eq!(config.explorer.width, 50);
        std::fs::remove_dir_all(&dir).ok();
    }
}

pub fn persist_theme_at(path: &Path, name: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = set_theme_value(&existing, name);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, updated)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn set_theme_value(text: &str, name: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let mut section_start = None;
    let mut section_end = lines.len();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            let header = trimmed.split('#').next().unwrap_or("").trim();
            match section_start {
                Some(_) => {
                    section_end = index;
                    break;
                }
                None if header == "[appearance]" => section_start = Some(index),
                None => {}
            }
        }
    }

    match section_start {
        Some(start) => {
            let key = (start + 1..section_end.min(lines.len())).find(|index| {
                let trimmed = lines[*index].trim_start();
                trimmed
                    .split('=')
                    .next()
                    .map(|name| name.trim() == "theme")
                    .unwrap_or(false)
            });
            match key {
                Some(index) => lines[index] = format!("theme = \"{escaped}\""),
                None => lines.insert(start + 1, format!("theme = \"{escaped}\"")),
            }
        }
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[appearance]".to_string());
            lines.push(format!("theme = \"{escaped}\""));
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod theme_persist_tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-persist-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join("config.toml")
    }

    #[test]
    fn replaces_theme_and_keeps_other_sections() {
        let path = temp("replace");
        std::fs::write(
            &path,
            "[editor]\ntab_size = 2\n\n[appearance]\n# look\nshow_status_bar = false\ntheme = \"dark\"\n\n[terminal]\nheight = 8\n",
        )
        .expect("write");
        persist_theme_at(&path, "mine").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("theme = \"mine\""), "{text}");
        assert!(text.contains("tab_size = 2"), "{text}");
        assert!(text.contains("height = 8"), "{text}");
        assert!(text.contains("# look"), "{text}");
        assert_eq!(text.matches("theme =").count(), 1, "{text}");
        assert!(!text.contains("theme = \"dark\""), "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn appends_the_section_when_missing() {
        let path = temp("append");
        std::fs::write(&path, "[editor]\ntab_size = 2\n").expect("write");
        persist_theme_at(&path, "light").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("[appearance]"), "{text}");
        assert!(text.contains("theme = \"light\""), "{text}");
        assert!(text.contains("tab_size = 2"), "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn inserts_the_key_into_an_existing_section() {
        let path = temp("insert");
        std::fs::write(
            &path,
            "[appearance]\nshow_status_bar = true\n[terminal]\nheight = 8\n",
        )
        .expect("write");
        persist_theme_at(&path, "high-contrast").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        let appearance = text.find("[appearance]").expect("section");
        let terminal = text.find("[terminal]").expect("terminal");
        let theme = text.find("theme = \"high-contrast\"").expect("theme");
        assert!(appearance < theme && theme < terminal, "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn creates_a_missing_file() {
        let path = temp("create");
        persist_theme_at(&path, "dark").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text, "[appearance]\ntheme = \"dark\"\n");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn escapes_quotes_in_the_name() {
        let path = temp("escape");
        persist_theme_at(&path, "a\"b").expect("persist");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("theme = \"a\\\"b\""), "{text}");
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }
}
