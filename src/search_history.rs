//! Persistent search queries and options, stored next to `config.toml`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::input::PromptKind;
use crate::search_options::Options;

const MAX_ENTRIES: usize = 50;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SearchHistory {
    pub options: Options,
    pub find: Vec<String>,
    pub find_in_files: Vec<String>,
    pub replace: Vec<String>,
    pub replace_in_files: Vec<String>,
}

impl SearchHistory {
    pub fn load(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: Option<&Path>) -> std::io::Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        // `fs::write` truncates first; a crash mid-save would lose the history.
        crate::editor::document::write_atomic(path, text.as_bytes())
    }

    pub fn list(&self, kind: PromptKind) -> &[String] {
        match kind {
            PromptKind::Find => &self.find,
            PromptKind::FindInFiles => &self.find_in_files,
            PromptKind::Replace => &self.replace,
            PromptKind::ReplaceInFiles => &self.replace_in_files,
            _ => &[],
        }
    }

    pub fn remember(&mut self, kind: PromptKind, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        let list = match kind {
            PromptKind::Find => &mut self.find,
            PromptKind::FindInFiles => &mut self.find_in_files,
            PromptKind::Replace => &mut self.replace,
            PromptKind::ReplaceInFiles => &mut self.replace_in_files,
            _ => return,
        };
        if let Some(index) = list.iter().position(|entry| entry == query) {
            list.remove(index);
        }
        list.insert(0, query.to_string());
        list.truncate(MAX_ENTRIES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trips_per_surface_and_keeps_options() {
        let dir = std::env::temp_dir().join(format!("i3-history-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("history.toml");
        let mut history = SearchHistory::default();
        history.remember(PromptKind::Find, "alpha");
        history.remember(PromptKind::Find, "beta");
        history.remember(PromptKind::Find, "alpha");
        history.remember(PromptKind::FindInFiles, "project");
        history.options.regex = true;
        history.save(Some(&path)).expect("save");

        let loaded = SearchHistory::load(Some(&path));
        assert_eq!(loaded.find, ["alpha", "beta"]);
        assert_eq!(loaded.find_in_files, ["project"]);
        assert!(loaded.options.regex);
        assert!(loaded.list(PromptKind::Replace).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
