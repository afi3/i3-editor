//! Semantic-memory primitives: chunking, cosine ranking, and the on-disk index.
//!
//! Test-gated until the embedding transport and the AI context hook land, so the
//! release build carries no dead code.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk shape version; a different number means "rebuild the index".
pub const VERSION: u32 = 1;

/// `<config>/i3-editor/semantic_index.json`.
pub fn default_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Ok(override_path) = TEST_PATH.lock()
        && let Some(path) = override_path.as_ref()
    {
        return Some(path.clone());
    }
    crate::config::config_dir().map(|dir| dir.join("semantic_index.json"))
}

#[cfg(test)]
static TEST_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Point the index at `path` for one test, or back at the real location.
#[cfg(test)]
pub(crate) fn use_path_for_test(path: Option<PathBuf>) {
    *TEST_PATH.lock().expect("semantic path override") = path;
}

/// One line-aligned slice of a file, `start_line` 0-based and inclusive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub path: PathBuf,
    pub start_line: usize,
    /// Exclusive.
    pub end_line: usize,
    pub text: String,
}

/// A chunk plus the vector the embedding model produced for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub chunk: Chunk,
    pub vector: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    /// Embedding model the vectors came from; mixing models is meaningless.
    pub model: String,
    pub entries: Vec<Entry>,
    /// sha256 of each file's text, so a rebuild skips unchanged files.
    #[serde(default)]
    pub files: std::collections::BTreeMap<PathBuf, String>,
}

impl Index {
    pub fn empty(model: impl Into<String>) -> Self {
        Self {
            version: VERSION,
            model: model.into(),
            entries: Vec::new(),
            files: std::collections::BTreeMap::new(),
        }
    }

    /// Read an index, treating a missing file, bad JSON, or a stale version as
    /// "no index" so callers can rebuild instead of failing.
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let index: Self = serde_json::from_str(&text).ok()?;
        (index.version == VERSION).then_some(index)
    }

    /// Write the index atomically and owner-only: vectors are derived from file
    /// contents, so they can reveal source text.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        crate::editor::document::write_atomic(path, text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

/// Split `text` into chunks of at most `lines` lines, dropping blank ones.
pub fn chunk(path: &Path, text: &str, lines: usize) -> Vec<Chunk> {
    let lines = lines.max(1);
    let all: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    for (index, block) in all.chunks(lines).enumerate() {
        let body = block.join("\n");
        if body.trim().is_empty() {
            continue;
        }
        let start_line = index * lines;
        chunks.push(Chunk {
            path: path.to_path_buf(),
            start_line,
            end_line: start_line + block.len(),
            text: body,
        });
    }
    chunks
}

/// Cosine similarity in `[-1, 1]`; 0.0 for an empty, zero, or mismatched pair.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut left = 0.0;
    let mut right = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        left += x * x;
        right += y * y;
    }
    if left == 0.0 || right == 0.0 {
        return 0.0;
    }
    dot / (left.sqrt() * right.sqrt())
}

/// Entries scoring at least `min_score`, best first, at most `k` of them.
/// Ties break on path so the order is deterministic.
pub fn top_k<'a>(index: &'a Index, query: &[f32], k: usize, min_score: f32) -> Vec<&'a Entry> {
    let mut scored: Vec<(f32, &Entry)> = index
        .entries
        .iter()
        .map(|entry| (cosine(query, &entry.vector), entry))
        .filter(|(score, _)| *score >= min_score)
        .collect();
    scored.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.chunk.path.cmp(&right.1.chunk.path))
            .then_with(|| left.1.chunk.start_line.cmp(&right.1.chunk.start_line))
    });
    scored.into_iter().take(k).map(|(_, entry)| entry).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Index {
        let mut index = Index::empty("test-model");
        index.entries.push(Entry {
            chunk: Chunk {
                path: PathBuf::from("a.rs"),
                start_line: 0,
                end_line: 2,
                text: "fn a() {}".to_string(),
            },
            vector: vec![1.0, 0.0],
        });
        index.entries.push(Entry {
            chunk: Chunk {
                path: PathBuf::from("b.rs"),
                start_line: 10,
                end_line: 12,
                text: "fn b() {}".to_string(),
            },
            vector: vec![0.0, 1.0],
        });
        index
    }

    #[test]
    fn chunking_is_line_aligned_and_drops_blanks() {
        let text = "one\ntwo\nthree\nfour\nfive\n";
        let chunks = chunk(Path::new("f.rs"), text, 2);
        assert_eq!(chunks.len(), 3, "{chunks:?}");
        assert_eq!(chunks[0].start_line, 0);
        assert_eq!(chunks[0].end_line, 2);
        assert_eq!(chunks[0].text, "one\ntwo");
        assert_eq!(chunks[2].start_line, 4);
        assert_eq!(chunks[2].text, "five");

        let blank = chunk(Path::new("f.rs"), "\n\n\n", 2);
        assert!(blank.is_empty(), "{blank:?}");
    }

    #[test]
    fn cosine_handles_identical_orthogonal_and_broken_pairs() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    #[test]
    fn ranking_respects_the_threshold_and_the_limit() {
        let index = index();
        let hits = top_k(&index, &[1.0, 0.0], 5, 0.5);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].chunk.path, PathBuf::from("a.rs"));

        let hits = top_k(&index, &[1.0, 0.0], 0, 0.0);
        assert!(hits.is_empty());

        let both = top_k(&index, &[1.0, 1.0], 5, 0.0);
        assert_eq!(both.len(), 2);
        assert_eq!(
            both[0].chunk.path,
            PathBuf::from("a.rs"),
            "ties break on path"
        );
    }

    #[test]
    fn the_index_round_trips_and_rejects_another_version() {
        let dir = std::env::temp_dir().join(format!("i3-semantic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("semantic_index.json");

        let written = index();
        written.save(&path).expect("save");
        let loaded = Index::load(&path).expect("load");
        assert_eq!(loaded, written);

        let mut stale = written.clone();
        stale.version = VERSION + 1;
        serde_json::to_string(&stale)
            .map(|text| std::fs::write(&path, text).expect("write"))
            .expect("json");
        assert!(
            Index::load(&path).is_none(),
            "a stale index must be ignored"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            written.save(&path).expect("save again");
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the index holds file text");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
