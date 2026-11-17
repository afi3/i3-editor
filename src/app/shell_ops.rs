//! Bottom terminal panel and theme switching.

use ratatui::crossterm::event::KeyEvent;

use crate::config::AppearanceConfig;
use crate::state::Focus;
use crate::theme::Theme;

use super::App;

impl App {
    pub(crate) fn toggle_shell(&mut self) {
        if self.state.shell.is_some() {
            self.state.shell = None;
            if matches!(self.state.focus, Focus::Shell) {
                let target = if self
                    .state
                    .workspaces
                    .active()
                    .layout
                    .contains(self.state.workspaces.active().focus)
                {
                    self.state.workspaces.active().focus
                } else {
                    self.state
                        .workspaces
                        .active()
                        .layout
                        .leaves()
                        .first()
                        .copied()
                        .unwrap_or(crate::state::PaneId(0))
                };
                self.state.focus = Focus::Pane(target);
            }
            self.notify("terminal closed");
            return;
        }

        let rows = self.state.config.terminal.height.saturating_sub(2).max(3);
        let cols = self.state.ui.area.width.saturating_sub(2).max(20);
        let cwd = self.state.explorer.root.clone();
        match crate::shell::ShellPanel::spawn(&cwd, rows, cols) {
            Ok(panel) => {
                self.state.shell = Some(panel);
                self.state.focus = Focus::Shell;
                self.notify("terminal opened");
            }
            Err(error) => self.notify(error.to_string()),
        }
    }

    /// Drain shell output; called once per loop iteration before drawing.
    pub(crate) fn pump_shell(&mut self) {
        if let Some(shell) = self.state.shell.as_mut() {
            shell.pump();
            if let Some(message) = shell.poll_exit() {
                let message = message.clone();
                self.notify(message);
            }
        }
    }

    pub(crate) fn resize_shell(&mut self) {
        let area = self.state.ui.shell;
        if area.width <= 2 || area.height <= 2 {
            return;
        }
        if let Some(shell) = self.state.shell.as_mut() {
            shell.resize(area.height - 2, area.width - 2);
        }
    }

    pub(crate) fn forward_to_shell(&mut self, key: KeyEvent) {
        if let Some(shell) = self.state.shell.as_mut() {
            shell.send_key(key);
        }
    }

    pub(crate) fn scroll_shell(&mut self, delta: isize) {
        if let Some(shell) = self.state.shell.as_mut() {
            shell.scroll(delta);
        }
        self.state.focus = Focus::Shell;
    }

    /// Apply a theme by preset or file name from the themes directory.
    pub(crate) fn apply_theme(&mut self, name: &str) {
        let appearance = AppearanceConfig {
            theme: name.to_string(),
            ..self.state.config.appearance.clone()
        };
        let (theme, warnings) = Theme::from_config(&appearance, &self.state.themes_dir);
        self.state.theme = theme;
        self.state.config.appearance.theme = name.to_string();
        match warnings.first() {
            Some(warning) => self.notify(warning.clone()),
            None => self.notify(format!("theme: {name}")),
        }
    }

    pub(crate) fn cycle_theme(&mut self, step: isize) {
        let names = Theme::names(&self.state.themes_dir);
        if names.is_empty() {
            return;
        }
        let current = self.state.config.appearance.theme.to_ascii_lowercase();
        let index = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&current))
            .unwrap_or(0) as isize;
        let next = (index + step).rem_euclid(names.len() as isize) as usize;
        let name = names[next].clone();
        self.apply_theme(&name);
    }
}
