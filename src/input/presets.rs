//! Built-in bindings and user overrides.
//!
//! Defaults deliberately avoid combinations terminals cannot distinguish:
//! no `Ctrl+Shift+<letter>`, no `Ctrl+H` (= Backspace), no `Alt+<letter>`.

use std::collections::{BTreeMap, HashMap};

use super::keymap::{KeyCombo, KeyMap};
use crate::command::{Command, Direction, Move};
use crate::config::KeysConfig;

fn bind(scope: &mut HashMap<KeyCombo, Command>, combo: &str, command: Command) {
    let parsed = KeyCombo::parse(combo).expect("built-in binding must parse");
    scope.insert(parsed, command);
}

impl KeyMap {
    /// Resolve defaults and `[keys.*]` overrides. Without keyboard
    /// enhancement the safe preset is mandatory: enhanced-only defaults would
    /// silently never fire.
    pub fn from_config(keys: &KeysConfig, enhanced: bool) -> (Self, Vec<String>) {
        let safe = !enhanced || keys.preset.as_deref() == Some("safe");
        let mut map = Self::preset(safe);
        let mut warnings = map.apply_config(keys);
        warnings.extend(map.shadowed());
        (map, warnings)
    }

    /// Actions whose default bindings only exist with keyboard enhancement.
    pub fn unavailable_without_enhancement() -> Vec<&'static str> {
        let full = Self::preset(false);
        let safe = Self::preset(true);
        let mut names: Vec<&'static str> = full
            .global
            .iter()
            .filter(|(combo, command)| safe.global.get(*combo) != Some(*command))
            .chain(
                full.editing
                    .iter()
                    .filter(|(combo, command)| safe.editing.get(*combo) != Some(*command)),
            )
            .chain(
                full.explorer
                    .iter()
                    .filter(|(combo, command)| safe.explorer.get(*combo) != Some(*command)),
            )
            .map(|(_, command)| command.name())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    pub fn preset(safe: bool) -> Self {
        let mut map = KeyMap::default();
        {
            let scope = &mut map.global;
            bind(scope, "ctrl+q", Command::Quit);
            bind(scope, "ctrl+s", Command::Save);
            bind(scope, "f2", Command::SavePrompt);
            bind(scope, "ctrl+o", Command::OpenPrompt);
            bind(scope, "ctrl+n", Command::NewPrompt);
            bind(scope, "ctrl+w", Command::CloseDocument);
            bind(scope, "ctrl+b", Command::ToggleExplorer);
            bind(scope, "ctrl+p", Command::QuickOpen);
            bind(scope, "f1", Command::Palette);
            bind(scope, "ctrl+g", Command::GoToLinePrompt);
            bind(scope, "ctrl+tab", Command::NextDocument);
            bind(scope, "backtab", Command::PrevDocument);
            bind(scope, "f3", Command::SplitLeftRight);
            bind(scope, "f4", Command::SplitTopBottom);
            bind(scope, "f5", Command::TerminalNew);
            bind(scope, "f11", Command::TerminalClose);
            bind(scope, "f6", Command::ClosePane);
            bind(scope, "f9", Command::ToggleFullscreen);
            bind(scope, "f10", Command::ToggleMoveMode);
            bind(scope, "f12", Command::ToggleTabbed);
            bind(scope, "ctrl+pagedown", Command::NextPaneTab);
            bind(scope, "ctrl+pageup", Command::PrevPaneTab);
            bind(scope, "f7", Command::NewWorkspace);
            bind(scope, "f8", Command::CloseWorkspace);
            if !safe {
                // Best-effort only: many terminals cannot report these.
                let combos = [
                    "ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4", "ctrl+5", "ctrl+6", "ctrl+7", "ctrl+8",
                    "ctrl+9",
                ];
                for (index, combo) in combos.iter().enumerate() {
                    bind(scope, combo, Command::SwitchWorkspace(index));
                }
            }
        }
        {
            let scope = &mut map.editing;
            bind(scope, "left", Command::MoveCursor(Move::Left));
            bind(scope, "right", Command::MoveCursor(Move::Right));
            bind(scope, "ctrl+left", Command::MoveCursor(Move::WordLeft));
            bind(scope, "ctrl+right", Command::MoveCursor(Move::WordRight));
            bind(scope, "up", Command::MoveCursor(Move::Up));
            bind(scope, "down", Command::MoveCursor(Move::Down));
            bind(scope, "home", Command::MoveCursor(Move::LineStart));
            bind(scope, "end", Command::MoveCursor(Move::LineEnd));
            bind(scope, "ctrl+home", Command::MoveCursor(Move::DocStart));
            bind(scope, "ctrl+end", Command::MoveCursor(Move::DocEnd));
            bind(scope, "pageup", Command::MoveCursor(Move::PageUp));
            bind(scope, "pagedown", Command::MoveCursor(Move::PageDown));
            bind(scope, "backspace", Command::Backspace);
            bind(scope, "delete", Command::DeleteForward);
            bind(scope, "enter", Command::InsertNewline);
            bind(scope, "tab", Command::InsertTab);
            bind(scope, "ctrl+z", Command::Undo);
            bind(scope, "ctrl+y", Command::Redo);
            bind(scope, "ctrl+a", Command::SelectAll);
            bind(scope, "ctrl+c", Command::Copy);
            bind(scope, "ctrl+x", Command::Cut);
            bind(scope, "ctrl+v", Command::Paste);
            bind(scope, "ctrl+f", Command::FindPrompt);
            bind(scope, "ctrl+r", Command::ReplacePrompt);
            bind(scope, "ctrl+d", Command::DuplicateLine);
            bind(scope, "ctrl+t", Command::ToggleComment);
            bind(scope, "ctrl+space", Command::LspCompletion);
            bind(scope, "ctrl+k", Command::LspHover);
            bind(
                scope,
                "ctrl+shift+left",
                Command::FocusPane(Direction::Left),
            );
            bind(
                scope,
                "ctrl+shift+right",
                Command::FocusPane(Direction::Right),
            );
            bind(scope, "ctrl+shift+up", Command::FocusPane(Direction::Up));
            bind(
                scope,
                "ctrl+shift+down",
                Command::FocusPane(Direction::Down),
            );
            bind(scope, "shift+left", Command::ExtendSelection(Move::Left));
            bind(scope, "shift+right", Command::ExtendSelection(Move::Right));
            bind(scope, "shift+up", Command::ExtendSelection(Move::Up));
            bind(scope, "shift+down", Command::ExtendSelection(Move::Down));
            bind(
                scope,
                "shift+home",
                Command::ExtendSelection(Move::LineStart),
            );
            bind(scope, "shift+end", Command::ExtendSelection(Move::LineEnd));
        }
        {
            let scope = &mut map.explorer;
            bind(scope, "up", Command::ExplorerUp);
            bind(scope, "down", Command::ExplorerDown);
            bind(scope, "enter", Command::ExplorerSelect);
            bind(scope, "right", Command::ExplorerExpand);
            bind(scope, "left", Command::ExplorerCollapse);
            bind(scope, "n", Command::ExplorerNewFile);
            bind(scope, "shift+n", Command::ExplorerNewDir);
            bind(scope, "r", Command::ExplorerRename);
            bind(scope, "d", Command::ExplorerDelete);
            bind(scope, "h", Command::ExplorerToggleHidden);
            bind(scope, "ctrl+r", Command::ExplorerRefresh);
        }
        if !safe {
            let scope = &mut map.editing;
            bind(scope, "alt+up", Command::MoveLinesUp);
            bind(scope, "alt+down", Command::MoveLinesDown);
        }
        map
    }

    /// Apply `[keys.*]` overrides. A bad entry is reported and skipped; it never
    /// removes the working default for the same action.
    pub fn apply_config(&mut self, keys: &KeysConfig) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut reserved = self.quit_combos();
        Self::apply_scope(
            &mut self.global,
            &keys.global,
            "global",
            keys,
            &reserved,
            &mut warnings,
        );
        reserved = self.quit_combos();
        Self::apply_scope(
            &mut self.editing,
            &keys.editing,
            "editing",
            keys,
            &reserved,
            &mut warnings,
        );
        reserved = self.quit_combos();
        Self::apply_scope(
            &mut self.explorer,
            &keys.explorer,
            "explorer",
            keys,
            &reserved,
            &mut warnings,
        );
        warnings
    }

    fn quit_combos(&self) -> Vec<KeyCombo> {
        self.global
            .iter()
            .filter(|(_, command)| **command == Command::Quit)
            .map(|(combo, _)| *combo)
            .collect()
    }

    fn apply_scope(
        scope: &mut HashMap<KeyCombo, Command>,
        overrides: &BTreeMap<String, String>,
        name: &str,
        keys: &KeysConfig,
        reserved: &[KeyCombo],
        warnings: &mut Vec<String>,
    ) {
        let mut seen: HashMap<KeyCombo, String> = HashMap::new();
        for (action, combo) in overrides {
            let prefix = keys.warning_prefix(name, action);
            let Some(command) = Command::from_name(action) else {
                warnings.push(format!("{prefix}unknown action {action:?}"));
                continue;
            };
            let parsed = match KeyCombo::parse(combo) {
                Ok(parsed) => parsed,
                Err(error) => {
                    warnings.push(format!("{prefix}{action}: {error}"));
                    continue;
                }
            };
            if command != Command::Quit && reserved.contains(&parsed) {
                warnings.push(format!(
                    "{prefix}{} is reserved for quit; {action} ignored",
                    parsed.label()
                ));
                continue;
            }
            if let Some(previous) = seen.get(&parsed) {
                warnings.push(format!(
                    "{prefix}{} is bound by {previous} and {action}; keeping {previous}",
                    parsed.label()
                ));
                continue;
            }
            seen.insert(parsed, action.clone());
            scope.insert(parsed, command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Focus;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn lookup(map: &KeyMap, text: &str) -> Option<Command> {
        map.resolve(Focus::Explorer, KeyCombo::parse(text).expect("combo"))
    }

    #[test]
    fn defaults_avoid_undecidable_combinations() {
        let map = KeyMap::preset(false);
        for combo in map
            .global
            .keys()
            .chain(map.editing.keys())
            .chain(map.explorer.keys())
        {
            let KeyCode::Char(ch) = combo.code else {
                continue;
            };
            let ctrl = combo.mods.contains(KeyModifiers::CONTROL);
            let shift = combo.mods.contains(KeyModifiers::SHIFT);
            assert!(!(ctrl && shift), "{} is not distinguishable", combo.label());
            if ctrl {
                assert!(
                    !matches!(ch, 'h' | 'i' | 'm' | 'j'),
                    "{} collides with a control key",
                    combo.label()
                );
            }
        }
        assert_eq!(lookup(&map, "ctrl+s"), Some(Command::Save));
        assert_eq!(lookup(&map, "f2"), Some(Command::SavePrompt));
    }

    #[test]
    fn lsp_completion_and_hover_have_default_bindings() {
        for safe in [false, true] {
            let map = KeyMap::preset(safe);
            assert_eq!(
                map.editing
                    .get(&KeyCombo::parse("ctrl+space").expect("combo")),
                Some(&Command::LspCompletion),
                "safe={safe}"
            );
            assert_eq!(
                map.editing.get(&KeyCombo::parse("ctrl+k").expect("combo")),
                Some(&Command::LspHover),
                "safe={safe}"
            );
        }
    }

    #[test]
    fn safe_preset_drops_best_effort_workspace_keys() {
        let safe = KeyMap::preset(true);
        assert!(
            !safe
                .global
                .values()
                .any(|command| matches!(command, Command::SwitchWorkspace(_)))
        );
        assert_eq!(lookup(&safe, "ctrl+s"), Some(Command::Save));
    }

    #[test]
    fn bad_overrides_warn_and_keep_defaults() {
        let mut keys = KeysConfig::default();
        keys.global.insert("save".to_string(), "ctrl+".to_string());
        keys.global
            .insert("nonsense".to_string(), "ctrl+k".to_string());
        keys.global.insert("quit".to_string(), "f12".to_string());
        let (map, warnings) = KeyMap::from_config(&keys, true);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(lookup(&map, "ctrl+s"), Some(Command::Save));
        assert_eq!(lookup(&map, "f12"), Some(Command::Quit));
    }

    #[test]
    fn without_enhancement_the_safe_preset_is_forced() {
        let keys = KeysConfig::default();
        let (map, _) = KeyMap::from_config(&keys, false);
        assert_eq!(lookup(&map, "ctrl+s"), Some(Command::Save));
        assert_eq!(lookup(&map, "ctrl+1"), None);
        assert!(
            KeyMap::unavailable_without_enhancement().contains(&"switch_workspace"),
            "{:?}",
            KeyMap::unavailable_without_enhancement()
        );
    }

    #[test]
    fn quit_bindings_cannot_be_shadowed() {
        let mut keys = KeysConfig::default();
        keys.global.insert("save".to_string(), "ctrl+q".to_string());
        keys.editing
            .insert("find".to_string(), "ctrl+q".to_string());
        let (map, warnings) = KeyMap::from_config(&keys, true);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().all(|warning| warning.contains("reserved")));
        assert_eq!(lookup(&map, "ctrl+q"), Some(Command::Quit));
        assert_eq!(map.editing.get(&KeyCombo::parse("ctrl+q").unwrap()), None);
        assert_eq!(
            map.editing.get(&KeyCombo::parse("ctrl+f").unwrap()),
            Some(&Command::FindPrompt)
        );
    }

    #[test]
    fn binding_warnings_name_the_config_line() {
        let dir = std::env::temp_dir().join(format!("i3-keys-line-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[keys.global]\nsave = \"ctrl+\"\n").expect("write");
        let (config, warnings) = crate::config::load_from(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        let (_, warnings) = KeyMap::from_config(&config.keys, true);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("save"), "{warnings:?}");
        assert!(
            warnings[0].contains(&format!("{}:2:", path.display())),
            "warning must name file and line: {warnings:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn duplicate_override_keeps_the_first_action() {
        let mut keys = KeysConfig::default();
        keys.global.insert("save".to_string(), "ctrl+e".to_string());
        keys.global.insert("quit".to_string(), "ctrl+e".to_string());
        let (map, warnings) = KeyMap::from_config(&keys, true);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // BTreeMap ordering decides which action is seen first.
        assert_eq!(lookup(&map, "ctrl+e"), Some(Command::Quit));
    }
}
