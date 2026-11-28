//! Lua plugin integration: snapshots, events, commands, and statusline.

use crate::input::KeyCombo;
use crate::plugins::Snapshot;

use super::App;

impl App {
    pub(crate) fn plugin_snapshot(&self) -> Snapshot {
        let document = self.state.active_document();
        let pane = self.state.focused_pane();
        let path = document.and_then(|document| document.path.clone());
        let line_column = match (pane, document) {
            (Some(pane), Some(document)) => {
                let line = document.line_of_char(pane.cursor);
                (
                    line,
                    pane.cursor.saturating_sub(document.char_of_line(line)),
                )
            }
            _ => (0, 0),
        };
        Snapshot {
            path: path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            language: path
                .as_deref()
                .map(crate::lsp::language_id)
                .unwrap_or("plaintext")
                .to_string(),
            line: line_column.0,
            column: line_column.1,
            selections: pane
                .map(|pane| {
                    pane.edit_ranges()
                        .into_iter()
                        .map(|range| (range.start, range.end))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Recompute statusline segments from every registered plugin.
    pub(crate) fn refresh_plugin_status(&mut self) {
        // Building the snapshot means formatting the path and copying the
        // selections; without plugins there is nothing to feed it to.
        if self.plugins.is_none() {
            self.state.plugin_status.clear();
            return;
        }
        let snapshot = self.plugin_snapshot();
        if let Some(plugins) = self.plugins.as_mut() {
            self.state.plugin_status = plugins.status_segments(&snapshot);
        }
        self.drain_plugin_warnings();
    }

    pub(crate) fn plugin_event(&mut self, event: &str) {
        if self.plugins.is_none() {
            return;
        }
        let snapshot = self.plugin_snapshot();
        if let Some(plugins) = self.plugins.as_mut() {
            plugins.fire(event, &snapshot);
        }
        self.drain_plugin_warnings();
        self.refresh_plugin_status();
    }

    pub(crate) fn run_plugin_command(&mut self, name: &str) -> bool {
        let snapshot = self.plugin_snapshot();
        let handled = self
            .plugins
            .as_mut()
            .is_some_and(|plugins| plugins.invoke_command(name, &snapshot));
        self.drain_plugin_warnings();
        self.refresh_plugin_status();
        handled
    }

    pub(crate) fn plugin_binding(&self, combo: KeyCombo) -> Option<String> {
        self.plugins
            .as_ref()
            .and_then(|plugins| plugins.binding(combo))
    }

    pub(crate) fn plugin_command_names(&self) -> Vec<String> {
        self.plugins
            .as_ref()
            .map(|plugins| plugins.command_names())
            .unwrap_or_default()
    }

    pub(crate) fn reload_plugins(&mut self) {
        let Some(plugins) = self.plugins.as_mut() else {
            return;
        };
        plugins.reload();
        let warnings = plugins.take_warnings();
        self.state.warnings.extend(warnings.clone());
        self.refresh_plugin_status();
        let count = self
            .plugins
            .as_ref()
            .map(|plugins| plugins.command_names().len())
            .unwrap_or(0);
        self.notify(format!("plugins reloaded ({count} commands)"));
    }

    fn drain_plugin_warnings(&mut self) {
        let warnings = self
            .plugins
            .as_mut()
            .map(|plugins| plugins.take_warnings())
            .unwrap_or_default();
        for warning in warnings {
            self.state.warnings.push(warning.clone());
            self.notify(warning);
        }
    }
}
