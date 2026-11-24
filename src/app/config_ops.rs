//! Reload configuration after the config file changes on disk.

use super::App;

impl App {
    pub(crate) fn reload_config(&mut self) {
        let Some(path) = self.state.config_path.clone() else {
            return;
        };
        let (mut config, mut warnings) = crate::config::load_from(&path);
        warnings.extend(crate::config::merge_managed_lsp(&mut config));
        let (keymap, key_warnings) = crate::input::KeyMap::from_config(&config.keys, self.enhanced);
        let (theme, theme_warnings) =
            crate::theme::Theme::from_config(&config.appearance, &self.state.themes_dir);
        warnings.extend(key_warnings);
        warnings.extend(theme_warnings);
        if let Some(message) = super::capability_warning(self.enhanced) {
            warnings.insert(0, message);
        }

        self.state.explorer.visible = config.explorer.visible;
        if self.state.explorer.show_hidden != config.explorer.show_hidden {
            self.state.explorer.show_hidden = config.explorer.show_hidden;
            self.state.explorer.refresh();
        }
        let lsp_changed = config.lsp != self.state.config.lsp;
        self.state.config = config;
        self.state.keymap = keymap;
        self.state.theme = theme;
        if lsp_changed {
            self.restart_lsp();
        }
        self.apply_document_indents();
        self.state.ui.message = if warnings.is_empty() {
            "config reloaded".to_string()
        } else {
            super::startup_message(&warnings)
        };
        self.state.warnings = warnings;
    }
}
