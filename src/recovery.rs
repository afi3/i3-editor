//! Crash recovery: swap files for dirty buffers.
//!
//! A swap file is written at most once per second while a buffer is dirty and
//! removed on the next write after a save. A crash leaves it behind, and the
//! next start offers to save, reload, or keep the recovered buffer. Graceful
//! exits remove the whole directory before the process goes away.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::editor::document::Document;
use crate::state::{DocumentId, Ids};

/// Buffers larger than this are not swapped; recovering a huge file is worse
/// than losing the last few edits.
const MAX_SWAP_BYTES: u64 = 16 << 20;

pub fn default_dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|dir| dir.join("swap"))
}

/// Write every dirty document and drop swaps for documents that are no longer
/// dirty. Callers treat failure as a warning, not a fatal error.
pub fn write_to<'a>(
    dir: &Path,
    documents: impl Iterator<Item = &'a Document>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut keep = HashSet::new();
    for document in documents {
        let Some(path) = &document.path else {
            continue;
        };
        if !document.dirty || document.rope.len_bytes() as u64 > MAX_SWAP_BYTES {
            continue;
        }
        let name = swap_name(path);
        let mut temporary = tempfile::Builder::new()
            .prefix(".i3-swap-")
            .tempfile_in(dir)?;
        std::io::Write::write_all(&mut temporary, document.rope.to_string().as_bytes())?;
        temporary
            .persist(dir.join(&name))
            .map_err(|error| error.error)?;
        keep.insert(name);
    }
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".swap") && !keep.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// Restore swaps into `documents`, opening any file that is not already open.
/// Returns the recovered documents in directory order.
pub fn recover_from(
    dir: &Path,
    documents: &mut HashMap<DocumentId, Document>,
    ids: &mut Ids,
) -> Vec<DocumentId> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut open_paths: HashMap<PathBuf, DocumentId> = documents
        .iter()
        .filter_map(|(id, document)| {
            document.path.as_ref().map(|path| {
                (
                    std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()),
                    *id,
                )
            })
        })
        .collect();
    let mut recovered = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(path) = path_from_name(&name) else {
            continue;
        };
        if !path.is_file() {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            let _ = std::fs::remove_file(entry.path());
            continue;
        };
        if text.len() as u64 > MAX_SWAP_BYTES {
            continue;
        }
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let id = match open_paths.get(&key).copied() {
            Some(id) => id,
            None => {
                let id = ids.document();
                match Document::open(id, path.clone()) {
                    Ok(document) => {
                        documents.insert(id, document);
                        open_paths.insert(key, id);
                        id
                    }
                    Err(_) => continue,
                }
            }
        };
        if let Some(document) = documents.get_mut(&id) {
            document.restore(&text);
            recovered.push(id);
        }
    }
    recovered
}

/// Remove the swap for one path, after the user resolved it.
pub fn clear(dir: &Path, path: &Path) {
    let _ = std::fs::remove_file(dir.join(swap_name(path)));
}

/// Remove every swap file; used on a graceful exit.
pub fn clear_all(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(".swap") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn swap_name(path: &Path) -> String {
    format!("{}.swap", hex(path.to_string_lossy().as_bytes()))
}

fn path_from_name(name: &str) -> Option<PathBuf> {
    let hex = name.strip_suffix(".swap")?;
    let bytes = unhex(hex)?;
    let text = String::from_utf8(bytes).ok()?;
    Some(PathBuf::from(text))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push(((high << 4) | low) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-swap-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn a_dirty_buffer_is_written_and_removed_after_save() {
        let dir = dir("dirty");
        let file = dir.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(4, "x", 0, (0, 0));

        write_to(&dir, [&document].into_iter()).expect("swap");
        let swaps: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".swap"))
            .collect();
        assert_eq!(swaps.len(), 1);

        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        let recovered = recover_from(&dir, &mut documents, &mut ids);
        assert_eq!(recovered.len(), 1);
        let restored = documents.get_mut(&recovered[0]).expect("document");
        assert_eq!(restored.rope.to_string(), "one\nx");
        assert!(restored.dirty);

        restored.save().expect("save");
        write_to(&dir, documents.values()).expect("prune");
        assert!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .flatten()
                .all(|entry| { !entry.file_name().to_string_lossy().ends_with(".swap") })
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_swap_without_its_file_is_dropped() {
        let dir = dir("orphan");
        let file = dir.join("gone.txt");
        std::fs::write(&file, "x\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(0, "y", 0, (0, 0));
        write_to(&dir, [&document].into_iter()).expect("swap");
        std::fs::remove_file(&file).expect("remove");

        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        assert!(recover_from(&dir, &mut documents, &mut ids).is_empty());
        assert!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .flatten()
                .all(|entry| { !entry.file_name().to_string_lossy().ends_with(".swap") })
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
