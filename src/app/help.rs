//! Keybinding help overlay built from the resolved keymap.

use std::collections::{HashMap, HashSet};

use ratatui::crossterm::event::KeyCode;

use crate::command::Command;
use crate::input::KeyCombo;
use crate::state::HelpView;

use super::App;

impl App {
    pub(crate) fn open_help(&mut self) {
        let mut lines = vec!["Esc or q closes this overlay".to_string(), String::new()];
        let mut bound = HashSet::new();
        append_bindings(&mut lines, "Global", &self.state.keymap.global, &mut bound);
        append_bindings(
            &mut lines,
            "Editing",
            &self.state.keymap.editing,
            &mut bound,
        );
        append_bindings(
            &mut lines,
            "Explorer",
            &self.state.keymap.explorer,
            &mut bound,
        );

        lines.push("Palette only".to_string());
        let mut palette: Vec<Command> = Command::palette()
            .into_iter()
            .filter(|command| !bound.contains(command.name()))
            .collect();
        palette.sort_by_key(|command| command.name());
        palette.dedup_by_key(|command| command.name());
        for command in palette {
            lines.push(format!(
                "{:<18} {} — {}",
                "-",
                command.name(),
                command.description()
            ));
        }
        self.state.help = Some(HelpView { lines, scroll: 0 });
        self.state.completions = None;
        self.state.hover = None;
    }

    pub(crate) fn handle_help_key(&mut self, combo: KeyCombo) -> bool {
        let Some(help) = self.state.help.as_mut() else {
            return false;
        };
        let last = help.lines.len().saturating_sub(1);
        let mut close = false;
        match combo.code {
            KeyCode::Up => help.scroll = help.scroll.saturating_sub(1),
            KeyCode::Down => help.scroll = (help.scroll + 1).min(last),
            KeyCode::PageUp => help.scroll = help.scroll.saturating_sub(20),
            KeyCode::PageDown => help.scroll = (help.scroll + 20).min(last),
            KeyCode::Home => help.scroll = 0,
            KeyCode::End => help.scroll = last,
            KeyCode::Esc | KeyCode::Char('q') => close = true,
            _ => {}
        }
        if close {
            self.state.help = None;
        }
        true
    }
}

fn append_bindings(
    lines: &mut Vec<String>,
    title: &str,
    bindings: &HashMap<KeyCombo, Command>,
    bound: &mut HashSet<&'static str>,
) {
    lines.push(title.to_string());
    let mut entries: Vec<(&KeyCombo, &Command)> = bindings.iter().collect();
    entries.sort_by(|left, right| {
        left.1
            .name()
            .cmp(right.1.name())
            .then_with(|| left.0.label().cmp(&right.0.label()))
    });
    for (combo, command) in entries {
        bound.insert(command.name());
        lines.push(format!(
            "{:<18} {} — {}",
            combo.label(),
            command.name(),
            command.description()
        ));
    }
    lines.push(String::new());
}
