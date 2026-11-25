//! Workspace metadata index for explicit AI context selection.
//!
//! The index stores paths and sizes only. File contents are read only when the
//! user explicitly selects a row and attaches it to the next prompt.

use std::path::Path;

use crate::persistent;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
const MAX_FILES: usize = 5_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default = "default_version")]
    pub version: u8,
    #[serde(default)]
    pub files: Vec<Entry>,
}

fn default_version() -> u8 {
    VERSION
}

pub fn build(root: &Path) -> Index {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .require_git(false)
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        files.push(Entry {
            path: relative,
            size: metadata.len(),
        });
        if files.len() >= MAX_FILES {
            break;
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Index {
        version: VERSION,
        files,
    }
}

pub fn load() -> Option<Index> {
    persistent::load_context::<Index>().filter(|index| index.version == VERSION)
}

pub fn save(index: &Index) -> Result<(), String> {
    persistent::save_context(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_records_metadata_only_and_honors_gitignore() {
        let root = std::env::temp_dir().join(format!(
            "i3-context-index-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").expect("ignore");
        std::fs::write(root.join("ignored.txt"), "secret").expect("ignored");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        let index = build(&root);
        assert!(index.files.iter().any(|entry| entry.path == "src/main.rs"));
        assert!(!index.files.iter().any(|entry| entry.path == "ignored.txt"));
        assert!(index.files.iter().all(|entry| entry.size > 0));
        std::fs::remove_dir_all(root).ok();
    }
}
