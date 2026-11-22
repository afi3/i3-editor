//! Bottom terminal panel and theme switching.

use std::path::PathBuf;

use ratatui::crossterm::event::KeyEvent;

use crate::config::AppearanceConfig;
use crate::error::Result;
use crate::input::PromptKind;
use crate::state::Focus;
use crate::theme::Theme;

use super::App;

impl App {
    pub(crate) fn toggle_shell(&mut self) {
        if !self.state.terminals.is_empty() {
            self.state.terminals = crate::shell::Terminals::default();
            if matches!(self.state.focus, Focus::Shell) {
                self.focus_back_to_pane();
            }
            self.notify("terminal closed");
            return;
        }
        self.open_terminal();
    }

    /// Spawn another shell in the panel and focus it.
    pub(crate) fn open_terminal(&mut self) {
        self.open_terminal_at(self.state.workspaces.active().terminal_dir.clone());
    }

    /// Spawn a shell in `cwd`, push it as a new tab, and focus the panel.
    pub(crate) fn open_terminal_at(&mut self, cwd: PathBuf) {
        let rows = self.state.config.terminal.height.saturating_sub(2).max(3);
        let cols = self.state.ui.area.width.saturating_sub(2).max(20);
        match crate::shell::ShellPanel::spawn(
            &cwd,
            rows,
            cols,
            self.state.config.terminal.shell.as_deref(),
            self.state.config.terminal.scrollback,
        ) {
            Ok(panel) => {
                let directory = panel.cwd().to_path_buf();
                self.state.terminals.push(panel);
                self.state.focus = Focus::Shell;
                let message = self.terminal_position();
                self.notify(format!("{message} in {}", directory.display()));
            }
            Err(error) => self.notify(error.to_string()),
        }
    }

    /// Close the active shell tab; the panel closes with its last tab.
    pub(crate) fn close_terminal(&mut self) {
        if self.state.terminals.is_empty() {
            return;
        }
        self.state.terminals.close_active();
        if self.state.terminals.is_empty() {
            if matches!(self.state.focus, Focus::Shell) {
                self.focus_back_to_pane();
            }
            self.notify("terminal closed");
        } else {
            let message = self.terminal_position();
            self.notify(message);
        }
    }

    pub(crate) fn cycle_terminal(&mut self, delta: isize) {
        if self.state.terminals.len() < 2 {
            return;
        }
        self.state.terminals.cycle(delta);
        self.state.focus = Focus::Shell;
        let message = self.terminal_position();
        self.notify(message);
    }

    pub(crate) fn rename_terminal_prompt(&mut self) {
        if self.state.terminals.is_empty() {
            self.notify("no terminal open");
            return;
        }
        self.open_prompt(PromptKind::TerminalRename);
    }

    pub(crate) fn rename_terminal(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            self.notify("invalid terminal name");
            return Ok(());
        }
        if self.state.terminals.rename_active(name) {
            self.notify(format!("terminal renamed to {name}"));
        } else {
            self.notify("no terminal open");
        }
        Ok(())
    }

    fn terminal_position(&self) -> String {
        let terminals = &self.state.terminals;
        let name = terminals
            .active()
            .map(crate::shell::ShellPanel::title)
            .unwrap_or("shell");
        format!(
            "terminal {}/{}: {name}",
            terminals.active_index() + 1,
            terminals.len()
        )
    }

    fn focus_back_to_pane(&mut self) {
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

    /// Drain every shell; called once per loop iteration before drawing.
    pub(crate) fn pump_shell(&mut self) {
        let mut messages = Vec::new();
        for shell in self.state.terminals.iter_mut() {
            shell.pump();
            if let Some(message) = shell.poll_exit() {
                messages.push(message);
            }
        }
        for message in messages {
            self.notify(message);
        }
    }

    pub(crate) fn resize_shell(&mut self) {
        let area = self.state.ui.shell;
        if area.width <= 2 || area.height <= 2 {
            return;
        }
        for shell in self.state.terminals.iter_mut() {
            shell.resize(area.height - 2, area.width - 2);
        }
    }

    pub(crate) fn paste_to_shell(&mut self, text: &str) {
        if let Some(shell) = self.state.terminals.active_mut() {
            shell.paste(text);
        }
    }

    pub(crate) fn forward_to_shell(&mut self, key: KeyEvent) {
        if let Some(shell) = self.state.terminals.active_mut() {
            shell.send_key(key);
        }
    }

    pub(crate) fn scroll_shell(&mut self, delta: isize) {
        if let Some(shell) = self.state.terminals.active_mut() {
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
        let mut message = warnings.first().cloned();
        if let Some(path) = self.state.config_path.clone()
            && let Err(error) = crate::config::persist_theme_at(&path, name)
        {
            message = Some(format!("theme applied, config not saved: {error}"));
        }
        match message {
            Some(message) => self.notify(message),
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
