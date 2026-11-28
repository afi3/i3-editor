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

    pub(crate) fn open_ai_provider_add(&mut self) {
        self.open_prompt(crate::input::PromptKind::AiProviderAdd);
    }

    pub(crate) fn open_ai_model_prompt(&mut self) {
        self.open_prompt(crate::input::PromptKind::AiModel);
    }

    pub(crate) fn open_ai_key_prompt(&mut self) {
        self.open_prompt(crate::input::PromptKind::AiKey);
    }

    /// Store a provider block from the add-provider prompt and activate it.
    pub(crate) fn submit_ai_provider_add(
        &mut self,
        prompt: crate::input::PromptState,
    ) -> crate::error::Result<()> {
        let name = prompt.fields.first().map_or("", String::as_str).trim();
        let base_url = prompt.fields.get(1).map_or("", String::as_str).trim();
        let model = prompt.fields.get(2).map_or("", String::as_str).trim();
        let api_key_env = prompt.fields.get(3).map_or("", String::as_str).trim();
        if name.is_empty() || base_url.is_empty() || model.is_empty() {
            self.notify("provider needs a name, base URL, and model");
            return Ok(());
        }
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            self.notify("provider name may use letters, digits, '-' and '_' only");
            return Ok(());
        }
        let name = name.to_string();
        let provider = crate::config::ProviderConfig {
            base_url: base_url.to_string(),
            model: model.to_string(),
            api_key_env: api_key_env.to_string(),
            ..Default::default()
        };
        match self.state.config_path.clone() {
            Some(path) => {
                match crate::config::persist_provider_at(
                    &path,
                    &name,
                    &provider.base_url,
                    &provider.model,
                    &provider.api_key_env,
                ) {
                    Ok(()) => {
                        self.state
                            .config
                            .ai
                            .providers
                            .insert(name.clone(), provider);
                        self.state.config.ai.activate(&name);
                        self.reload_config();
                        let model = self.state.config.ai.model.clone();
                        self.notify(format!("AI provider: {name} ({model})"));
                    }
                    Err(error) => self.notify(format!("provider not saved: {error}")),
                }
            }
            None => {
                self.state
                    .config
                    .ai
                    .providers
                    .insert(name.clone(), provider);
                self.state.config.ai.activate(&name);
                self.notify(format!(
                    "AI provider: {name} ({model}) (memory only; no config path)"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn submit_ai_model(
        &mut self,
        prompt: crate::input::PromptState,
    ) -> crate::error::Result<()> {
        let model = prompt
            .fields
            .first()
            .map_or("", String::as_str)
            .trim()
            .to_string();
        if model.is_empty() {
            self.notify("model cannot be empty");
            return Ok(());
        }
        self.persist_ai_setting("model", &model, "AI model");
        Ok(())
    }

    pub(crate) fn submit_ai_key(
        &mut self,
        prompt: crate::input::PromptState,
    ) -> crate::error::Result<()> {
        let env = prompt
            .fields
            .first()
            .map_or("", String::as_str)
            .trim()
            .to_string();
        if !env.is_empty()
            && !env
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            self.notify("environment variable names use letters, digits and '_'");
            return Ok(());
        }
        self.persist_ai_setting("api_key_env", &env, "AI key environment");
        Ok(())
    }

    /// Write one `[ai]` setting for the active provider and apply it in memory.
    fn persist_ai_setting(&mut self, key: &str, value: &str, label: &str) {
        let saved = self.state.config_path.clone().map_or(Ok(()), |path| {
            crate::config::persist_ai_value_at(&path, key, value)
        });
        let active = self.state.config.ai.active_provider.clone();
        if !active.is_empty()
            && let Some(provider) = self.state.config.ai.providers.get_mut(&active)
        {
            match key {
                "model" => provider.model = value.to_string(),
                "api_key_env" => provider.api_key_env = value.to_string(),
                _ => {}
            }
        }
        match key {
            "model" => self.state.config.ai.model = value.to_string(),
            "api_key_env" => self.state.config.ai.api_key_env = value.to_string(),
            _ => {}
        }
        let shown = if value.is_empty() { "(empty)" } else { value };
        match saved {
            Ok(()) => self.notify(format!("{label}: {shown}")),
            Err(error) => self.notify(format!(
                "{label}: {shown} (applied in memory; config.toml not saved: {error})"
            )),
        }
    }

    /// Candidates for the subscription picker: `server uri`, one per row.
    pub(crate) fn fill_subscription_candidates(&mut self, prompt: &mut crate::input::PromptState) {
        self.mcp_subscription_picks = self.mcp_subscriptions.iter().cloned().collect();
        let candidates: Vec<String> = self
            .mcp_subscription_picks
            .iter()
            .map(|(server, uri)| format!("{server} {uri}"))
            .collect();
        if candidates.is_empty() {
            self.notify("no MCP resources are subscribed");
            return;
        }
        prompt.set_candidates(candidates);
    }

    /// Release one subscription chosen from the picker, by row index.
    pub(crate) fn unsubscribe_mcp_subscription(&mut self, index: usize) {
        let Some((server, uri)) = self.mcp_subscription_picks.get(index).cloned() else {
            return;
        };
        let key = (server.clone(), uri.clone());
        if !self.mcp_subscriptions.remove(&key) {
            self.notify(format!("{uri} is not subscribed"));
            return;
        }
        if let Some(mcp) = self.mcp.as_mut() {
            let _ = mcp.unsubscribe_resource(&server, &uri);
        }
        self.notify(format!("released {uri} from {server}"));
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
        let mcp_changed = config.mcp.servers != self.state.config.mcp.servers;
        self.state.config = config;
        self.state.keymap = keymap;
        self.state.theme = theme;
        if lsp_changed {
            self.restart_lsp();
        }
        // Every save of config.toml used to restart every MCP server, killing
        // in-flight tool calls and subscriptions for an unrelated edit.
        if mcp_changed {
            if let Some(mcp) = self.mcp.as_mut() {
                mcp.restart(
                    self.state.config.mcp.servers.clone(),
                    self.state.explorer.root.clone(),
                );
            }
            self.mcp_subscriptions.clear();
            self.mcp_resource_view = None;
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
