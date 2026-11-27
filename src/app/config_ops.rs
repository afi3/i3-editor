//! Reload configuration after the config file changes on disk.

use super::App;

impl App {
    /// Activate a configured provider and remember it in `config.toml`.
    pub(crate) fn select_ai_provider(&mut self, name: &str) {
        if !self.state.config.ai.activate(name) {
            self.notify(format!("unknown AI provider [{name}]"));
            return;
        }
        let model = self.state.config.ai.model.clone();
        let saved = self.state.config_path.clone().map_or(Ok(()), |path| {
            crate::config::persist_active_provider_at(&path, name)
        });
        match saved {
            Ok(()) => self.notify(format!("AI provider: {name} ({model})")),
            Err(error) => self.notify(format!(
                "AI provider: {name} ({model}); config.toml not saved: {error}"
            )),
        }
    }

    /// Palette command: release every MCP resource subscription this session
    /// holds, so servers stop pushing updates for resources nobody views.
    pub(crate) fn unsubscribe_all_mcp_resources(&mut self) {
        if self.mcp_subscriptions.is_empty() {
            self.notify("no MCP resources are subscribed");
            return;
        }
        let subscriptions: Vec<(String, String)> = self.mcp_subscriptions.iter().cloned().collect();
        let mut released = 0usize;
        for (server, uri) in &subscriptions {
            if let Some(mcp) = self.mcp.as_mut()
                && mcp.unsubscribe_resource(server, uri).is_ok()
            {
                released += 1;
            }
            self.mcp_subscriptions
                .remove(&(server.clone(), uri.clone()));
        }
        self.notify(format!(
            "released {released} of {} MCP resource subscription(s)",
            subscriptions.len()
        ));
    }

    /// Candidates for the provider picker; the list is the registry order.
    pub(crate) fn fill_provider_candidates(&mut self, prompt: &mut crate::input::PromptState) {
        let names: Vec<String> = self.state.config.ai.providers.keys().cloned().collect();
        if names.is_empty() {
            self.notify("no [ai.providers.*] entries in config.toml");
            return;
        }
        prompt.set_candidates(names);
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
