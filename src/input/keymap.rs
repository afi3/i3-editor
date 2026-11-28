//! Key combinations: parsing, normalization, and labels.

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::command::Command;
use crate::error::{Error, Result};
use crate::state::Focus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

fn bad(entry: &str, reason: impl Into<String>) -> Error {
    Error::InvalidBinding {
        entry: entry.to_string(),
        reason: reason.into(),
    }
}

impl KeyCombo {
    pub fn parse(text: &str) -> Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(bad(text, "empty binding"));
        }
        let mut pieces: Vec<&str> = trimmed.split('+').map(str::trim).collect();
        let key = pieces
            .pop()
            .filter(|piece| !piece.is_empty())
            .ok_or_else(|| bad(text, "no key after the last '+'"))?;
        let mut mods = KeyModifiers::NONE;
        for piece in pieces {
            match piece.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "alt" | "opt" | "option" | "meta" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                "" => return Err(bad(text, "empty modifier segment")),
                other => return Err(bad(text, format!("unknown modifier {other:?}"))),
            }
        }
        let code = parse_code(key, &mut mods)
            .ok_or_else(|| bad(text, format!("unknown key name {key:?}")))?;
        Ok(Self { code, mods })
    }

    /// Normalize a terminal event into the form used by binding tables.
    ///
    /// Terminals report shifted letters as uppercase chars, and `BackTab`
    /// always carries SHIFT, so both are folded before lookup.
    pub fn from_event(event: KeyEvent) -> Option<Self> {
        if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        let mut mods = event.modifiers;
        let code = match event.code {
            // QTerminal and other Unix terminals send Backspace as Ctrl+H
            // (0x08); treat it as Backspace just like 0x7f.
            KeyCode::Char(ch)
                if mods.contains(KeyModifiers::CONTROL)
                    && !mods.contains(KeyModifiers::ALT)
                    && ch.eq_ignore_ascii_case(&'h') =>
            {
                mods.remove(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
                KeyCode::Backspace
            }
            KeyCode::Char('\u{7f}') => {
                mods.remove(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
                KeyCode::Backspace
            }
            KeyCode::Char(ch) => {
                if ch.is_ascii_uppercase() {
                    mods |= KeyModifiers::SHIFT;
                    KeyCode::Char(ch.to_ascii_lowercase())
                } else {
                    KeyCode::Char(ch)
                }
            }
            KeyCode::Backspace | KeyCode::Delete => {
                // Some terminals attach a spurious Shift/Ctrl to these keys;
                // editing still means backward/forward deletion.
                mods.remove(KeyModifiers::CONTROL | KeyModifiers::SHIFT | KeyModifiers::ALT);
                event.code
            }
            KeyCode::BackTab => {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            other => other,
        };
        Some(Self { code, mods })
    }

    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) && !matches!(self.code, KeyCode::BackTab) {
            out.push_str("Shift+");
        }
        out.push_str(&match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(ch) => ch.to_uppercase().to_string(),
            KeyCode::F(number) => format!("F{number}"),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "BackTab".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Insert => "Insert".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            other => format!("{other:?}"),
        });
        out
    }
}

fn parse_code(part: &str, mods: &mut KeyModifiers) -> Option<KeyCode> {
    let lower = part.to_ascii_lowercase();
    Some(match lower.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "space" => KeyCode::Char(' '),
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        _ => {
            if let Some(rest) = lower.strip_prefix('f')
                && let Ok(number) = rest.parse::<u8>()
                && (1..=24).contains(&number)
            {
                return Some(KeyCode::F(number));
            }
            if part.chars().count() == 1 {
                let ch = part.chars().next()?;
                if ch.is_ascii_uppercase() {
                    *mods |= KeyModifiers::SHIFT;
                    KeyCode::Char(ch.to_ascii_lowercase())
                } else {
                    KeyCode::Char(ch)
                }
            } else {
                return None;
            }
        }
    })
}

#[derive(Debug, Default)]
pub struct KeyMap {
    pub global: HashMap<KeyCombo, Command>,
    pub editing: HashMap<KeyCombo, Command>,
    pub explorer: HashMap<KeyCombo, Command>,
}

impl KeyMap {
    /// Resolution order: prompt (handled by the caller), focused surface, global.
    pub fn resolve(&self, focus: Focus, combo: KeyCombo) -> Option<Command> {
        match focus {
            Focus::Pane(_) => self
                .editing
                .get(&combo)
                .or_else(|| self.global.get(&combo))
                .cloned(),
            Focus::Explorer => self
                .explorer
                .get(&combo)
                .or_else(|| self.global.get(&combo))
                .cloned(),
            Focus::Shell => self.global.get(&combo).cloned(),
        }
    }

    /// Binding that currently wins for `command` on the focused surface.
    pub fn active_binding(&self, focus: Focus, command: &Command) -> Option<String> {
        let scope = match focus {
            Focus::Pane(_) => Some(&self.editing),
            Focus::Explorer => Some(&self.explorer),
            Focus::Shell => None,
        };
        if let Some(label) = scope.and_then(|bindings| binding_label(bindings, command)) {
            return Some(label);
        }
        binding_label(&self.global, command)
    }

    /// Bindings that a focused scope hides from the global scope.
    pub fn shadowed(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (scope, where_it_runs) in [
            (&self.editing, "when a pane is focused"),
            (&self.explorer, "when the explorer is focused"),
        ] {
            for (combo, command) in scope {
                if let Some(global) = self.global.get(combo) {
                    out.push(format!(
                        "{} runs {} {where_it_runs} and {} globally",
                        combo.label(),
                        command.name(),
                        global.name()
                    ));
                }
            }
        }
        out
    }

    pub fn labels(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .global
            .iter()
            .chain(self.editing.iter())
            .chain(self.explorer.iter())
            .map(|(combo, command)| format!("{} -> {}", combo.label(), command.name()))
            .collect();
        out.sort();
        out
    }
}

fn binding_label(bindings: &HashMap<KeyCombo, Command>, command: &Command) -> Option<String> {
    bindings
        .iter()
        .filter(|(_, bound)| *bound == command)
        .map(|(combo, _)| combo.label())
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;

    fn combo(text: &str) -> KeyCombo {
        KeyCombo::parse(text).expect("valid combo")
    }

    #[test]
    fn explorer_overrides_that_hide_global_bindings_are_reported() {
        let mut map = KeyMap::preset(false);
        map.explorer.insert(
            KeyCombo::parse("ctrl+p").expect("combo"),
            Command::ExplorerRefresh,
        );
        assert!(
            map.shadowed()
                .iter()
                .any(|line| line.contains("explorer is focused")),
            "{:?}",
            map.shadowed()
        );
    }

    #[test]
    fn parses_modifiers_and_keys() {
        let parsed = combo("ctrl+shift+left");
        assert_eq!(parsed.code, KeyCode::Left);
        assert!(parsed.mods.contains(KeyModifiers::CONTROL));
        assert!(parsed.mods.contains(KeyModifiers::SHIFT));
        assert_eq!(parsed.label(), "Ctrl+Shift+Left");

        let function = combo("f7");
        assert_eq!(function.code, KeyCode::F(7));
        assert_eq!(function.label(), "F7");

        let upper = combo("ctrl+S");
        assert_eq!(upper.code, KeyCode::Char('s'));
        assert!(upper.mods.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn rejects_unknown_names() {
        assert!(KeyCombo::parse("ctrl+").is_err());
        assert!(KeyCombo::parse("hyper+x").is_err());
        assert!(KeyCombo::parse("nonsense").is_err());
        assert!(KeyCombo::parse("").is_err());
    }

    #[test]
    fn terminal_events_normalize_to_bindings() {
        let event = KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT);
        let normalized = KeyCombo::from_event(event).expect("press event");
        assert_eq!(normalized.label(), "Shift+S");
        assert_eq!(normalized.code, KeyCode::Char('s'));

        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            KeyCombo::from_event(backtab).map(|c| c.label()),
            Some("BackTab".to_string())
        );

        let release = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(KeyCombo::from_event(release).is_none());

        let ctrl_h = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert_eq!(
            KeyCombo::from_event(ctrl_h).map(|combo| combo.code),
            Some(KeyCode::Backspace)
        );

        let modified_delete = KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT);
        assert_eq!(
            KeyCombo::from_event(modified_delete).map(|combo| (combo.code, combo.mods)),
            Some((KeyCode::Delete, KeyModifiers::NONE))
        );
    }

    #[test]
    fn active_binding_prefers_the_focused_scope() {
        let map = KeyMap::preset(false);
        assert_eq!(
            map.active_binding(Focus::Pane(crate::state::PaneId(1)), &Command::Save),
            Some("Ctrl+S".to_string())
        );
        assert_eq!(
            map.active_binding(Focus::Explorer, &Command::ExplorerRename),
            Some("R".to_string())
        );
        assert_eq!(
            map.active_binding(Focus::Explorer, &Command::Save),
            Some("Ctrl+S".to_string())
        );
        assert_eq!(map.active_binding(Focus::Shell, &Command::Copy), None);
    }

    #[test]
    fn focused_scope_wins_over_global() {
        let mut map = KeyMap::default();
        map.global.insert(combo("ctrl+k"), Command::Quit);
        map.editing.insert(combo("ctrl+k"), Command::Copy);
        assert_eq!(
            map.resolve(Focus::Pane(crate::state::PaneId(1)), combo("ctrl+k")),
            Some(Command::Copy)
        );
        assert_eq!(
            map.resolve(Focus::Explorer, combo("ctrl+k")),
            Some(Command::Quit)
        );
        assert_eq!(map.shadowed().len(), 1);
    }
}
