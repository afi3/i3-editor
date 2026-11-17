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
