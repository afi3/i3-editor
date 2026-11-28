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

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

pub fn select(index: &Index, query: &str, limit: usize) -> Vec<Entry> {
    let terms: Vec<String> = query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .map(str::to_ascii_lowercase)
        .filter(|term| term.len() >= 3)
        .collect();
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &Entry)> = index
        .files
        .iter()
        .filter_map(|entry| {
            let path = entry.path.to_ascii_lowercase();
            let mut score = 0usize;
            for term in &terms {
                if path.contains(term) {
                    score += 1;
                }
            }
            (score > 0).then_some((score, entry))
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            // A file near the root is the likelier target than a tiny one buried
            // in a vendor directory; size only separates equal depths.
            .then_with(|| depth(&left.1.path).cmp(&depth(&right.1.path)))
            .then_with(|| left.1.size.cmp(&right.1.size))
            .then_with(|| left.1.path.cmp(&right.1.path))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry.clone())
        .collect()
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

#[cfg(test)]
mod select_tests {
    use super::*;

    fn entry(path: &str, size: u64) -> Entry {
        Entry {
            path: path.to_string(),
            size,
        }
    }

    #[test]
    fn shallow_paths_win_equal_scores() {
        let index = Index {
            version: VERSION,
            files: vec![
                entry("vendor/deep/main.rs", 10),
                entry("main.rs", 500),
                entry("src/main.rs", 200),
            ],
        };
        let picked = select(&index, "main", 3);
        let order: Vec<&str> = picked.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(order, vec!["main.rs", "src/main.rs", "vendor/deep/main.rs"]);
    }

    #[test]
    fn equal_depths_still_prefer_smaller_files() {
        let index = Index {
            version: VERSION,
            files: vec![entry("a/alpha.rs", 900), entry("b/alpha.rs", 100)],
        };
        let picked = select(&index, "alpha", 2);
        let order: Vec<&str> = picked.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(order, vec!["b/alpha.rs", "a/alpha.rs"]);
    }
}
