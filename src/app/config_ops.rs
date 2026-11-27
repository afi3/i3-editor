//! Reload configuration after the config file changes on disk.

use super::App;

impl App {
    /// Palette command: switch to the next configured AI provider.
    pub(crate) fn next_ai_provider(&mut self) -> crate::error::Result<()> {
        if self.state.config.ai.providers.is_empty() {
            self.notify("no [ai.providers.*] entries in config.toml");
            return Ok(());
        }
        let names: Vec<String> = self.state.config.ai.providers.keys().cloned().collect();
        let current = names
            .iter()
            .position(|name| *name == self.state.config.ai.active_provider);
        let next = current.map_or(0, |index| (index + 1) % names.len());
        let name = names[next].clone();
        if self.state.config.ai.activate(&name) {
            let model = self.state.config.ai.model.clone();
            let saved = self.state.config_path.clone().map_or(Ok(()), |path| {
                crate::config::persist_active_provider_at(&path, &name)
            });
            match saved {
                Ok(()) => self.notify(format!("AI provider: {name} ({model})")),
                Err(error) => self.notify(format!(
                    "AI provider: {name} ({model}); config.toml not saved: {error}"
                )),
            }
        }
        Ok(())
    }

    pub(crate) fn reload_config(&mut self) {
        let Some(path) = self.state.config_path.clone() else {
            return;
        };
        let (mut config, mut warnings) = crate::config::load_from(&path);
        warnings.extend(crate::config::merge_managed_lsp(&mut config));
        warnings.extend(crate::config::merge_managed_mcp(&mut config));
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
        if let Some(mcp) = self.mcp.as_mut() {
            mcp.restart(
                self.state.config.mcp.servers.clone(),
                self.state.explorer.root.clone(),
            );
        }
        self.mcp_subscriptions.clear();
        self.mcp_resource_view = None;
        self.apply_document_indents();
        self.state.ui.message = if warnings.is_empty() {
            "config reloaded".to_string()
        } else {
            super::startup_message(&warnings)
        };
        self.state.warnings = warnings;
    }
}
