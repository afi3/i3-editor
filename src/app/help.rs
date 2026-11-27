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
        self.state.rules_view = None;
        self.state.trace_view = None;
        self.state.completions = None;
        self.state.hover = None;
    }

    pub(crate) fn handle_help_key(&mut self, combo: KeyCombo) -> bool {
        let Some(help) = self.state.help.as_mut() else {
            return false;
        };
        if read_only_key(help, combo) {
            self.state.help = None;
        }
        true
    }

    pub(crate) fn handle_rules_key(&mut self, combo: KeyCombo) -> bool {
        let Some(view) = self.state.rules_view.as_mut() else {
            return false;
        };
        if read_only_key(view, combo) {
            self.state.rules_view = None;
        }
        true
    }

    pub(crate) fn handle_trace_key(&mut self, combo: KeyCombo) -> bool {
        let Some(view) = self.state.trace_view.as_mut() else {
            return false;
        };
        if read_only_key(view, combo) {
            self.state.trace_view = None;
        }
        true
    }
}

pub(crate) trait ScrollView {
    fn line_count(&self) -> usize;
    fn scroll_mut(&mut self) -> &mut usize;
}

impl ScrollView for HelpView {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn scroll_mut(&mut self) -> &mut usize {
        &mut self.scroll
    }
}

impl ScrollView for crate::state::DiffView {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn scroll_mut(&mut self) -> &mut usize {
        &mut self.scroll
    }
}

pub(crate) fn scroll_view<V: ScrollView>(view: &mut V, steps: isize) {
    let last = view.line_count().saturating_sub(1);
    let amount = steps.unsigned_abs();
    let scroll = view.scroll_mut();
    *scroll = if steps < 0 {
        scroll.saturating_sub(amount)
    } else {
        (*scroll + amount).min(last)
    };
}

pub(crate) fn read_only_key<V: ScrollView>(view: &mut V, combo: KeyCombo) -> bool {
    let last = view.line_count().saturating_sub(1);
    let scroll = view.scroll_mut();
    match combo.code {
        KeyCode::Up => *scroll = scroll.saturating_sub(1),
        KeyCode::Down => *scroll = (*scroll + 1).min(last),
        KeyCode::PageUp => *scroll = scroll.saturating_sub(20),
        KeyCode::PageDown => *scroll = (*scroll + 20).min(last),
        KeyCode::Home => *scroll = 0,
        KeyCode::End => *scroll = last,
        KeyCode::Esc | KeyCode::Char('q') => return true,
        _ => {}
    }
    false
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
