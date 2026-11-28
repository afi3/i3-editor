//! Crash recovery: swap files for dirty buffers.
//!
//! Every running editor owns a subdirectory of `<config>/swap` named from its
//! pid and start time and guarded by an advisory lock on `lock`; a running
//! owner is recognised by that lock being held. Inside the subdirectory a swap
//! file is written at most once per second while a buffer is dirty and removed
//! on the next write after a save. A crash leaves the subdirectory behind, and
//! the next start adopts swaps from directories whose owner is gone while
//! leaving the swaps of running instances alone. A graceful exit removes only
//! its own subdirectory.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::editor::document::Document;
use crate::state::{DocumentId, Ids};

/// Buffers larger than this are not swapped; recovering a huge file is worse
/// than losing the last few edits.
const MAX_SWAP_BYTES: u64 = 16 << 20;

const LOCK_NAME: &str = "lock";

pub fn default_dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|dir| dir.join("swap"))
}

/// Directories this process holds open. A dropped instance's directory cannot be
/// told apart from a live one by its lock alone: both belong to this process, and
/// the lock probe only distinguishes processes reliably.
fn live_dirs() -> &'static Mutex<HashSet<PathBuf>> {
    static LIVE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn register_live(dir: &Path) {
    if let Ok(mut live) = live_dirs().lock() {
        live.insert(dir.to_path_buf());
    }
}

fn unregister_live(dir: &Path) {
    if let Ok(mut live) = live_dirs().lock() {
        live.remove(dir);
    }
}

fn is_live(dir: &Path) -> bool {
    live_dirs()
        .lock()
        .map(|live| live.contains(dir))
        .unwrap_or(false)
}

/// The pid embedded in an instance directory name, if it has one.
fn dir_pid(dir: &Path) -> Option<u32> {
    dir.file_name()?
        .to_string_lossy()
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// This process's swap directory, owned for as long as the lock is held.
pub struct Instance {
    dir: PathBuf,
    /// Held for the process lifetime; dropping it releases the lock.
    _lock: File,
}

impl Instance {
    pub fn claim(base: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(base)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = claim_dir(base, stamp)?;
        // Registered before the lock: a concurrent scan must treat a directory
        // that is mid-claim as alive.
        register_live(&dir);
        let lock = match File::options()
            .create_new(true)
            .write(true)
            .open(dir.join(LOCK_NAME))
        {
            Ok(lock) => lock,
            Err(error) => {
                unregister_live(&dir);
                return Err(error);
            }
        };
        match lock.try_lock() {
            Ok(()) => Ok(Self { dir, _lock: lock }),
            Err(std::fs::TryLockError::WouldBlock) => {
                unregister_live(&dir);
                Err(std::io::Error::other("swap directory is already locked"))
            }
            Err(std::fs::TryLockError::Error(error)) => {
                unregister_live(&dir);
                Err(error)
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Remove this instance's swaps after a graceful exit. Dropping `self`
    /// releases the lock and deregisters the directory.
    pub fn release(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unregister_live(&self.dir);
    }
}

fn claim_dir(base: &Path, stamp: u128) -> std::io::Result<PathBuf> {
    let mut suffix = 0u32;
    loop {
        let candidate = base.join(format!("{}-{stamp}-{suffix}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && suffix < 64 => {
                suffix += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Write every dirty document and drop swaps for documents that are no longer
/// dirty. `dir` is this instance's own subdirectory. Callers treat failure as a
/// warning, not a fatal error.
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
        if !document.dirty || document.rope().len_bytes() as u64 > MAX_SWAP_BYTES {
            continue;
        }
        let name = swap_name(path);
        let mut temporary = tempfile::Builder::new()
            .prefix(".i3-swap-")
            .tempfile_in(dir)?;
        std::io::Write::write_all(&mut temporary, document.rope().to_string().as_bytes())?;
        temporary
            .persist(dir.join(&name))
            .map_err(|error| error.error)?;
        // The disk stamp lives beside the swap, so the payload stays plain
        // buffer text and an older build can still read it after a downgrade.
        let _ = std::fs::write(dir.join(stamp_name(path)), stamp_text(document));
        keep.insert(name);
    }
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".swap") && !keep.contains(&name) {
            let path = entry.path();
            let _ = std::fs::remove_file(path.with_extension("stamp"));
            let _ = std::fs::remove_file(path);
        } else if let Some(base) = name.strip_suffix(".stamp")
            && !keep.contains(&format!("{base}.swap"))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// Replay swaps left by instances that are gone into `documents`, opening any
/// file that is not already open. Swaps of a running instance are left alone.
/// Adopted swaps move into `own`, so one directory owns them from now on.
pub fn recover_from(
    base: &Path,
    own: &Path,
    documents: &mut HashMap<DocumentId, Document>,
    ids: &mut Ids,
) -> Vec<DocumentId> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    // Swaps written by versions without instance directories sit in `base`.
    let mut sources = vec![base.to_path_buf()];
    let mut abandoned = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path == own || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if owner_gone(&path) {
            sources.push(path.clone());
            abandoned.push(path);
        }
    }

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
    for dir in sources {
        recover_dir(&dir, own, documents, ids, &mut open_paths, &mut recovered);
    }
    for dir in abandoned {
        if !has_swaps(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    recovered
}

fn recover_dir(
    dir: &Path,
    own: &Path,
    documents: &mut HashMap<DocumentId, Document>,
    ids: &mut Ids,
    open_paths: &mut HashMap<PathBuf, DocumentId>,
    recovered: &mut Vec<DocumentId>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(path) = path_from_name(&name) else {
            continue;
        };
        if !path.is_file() {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_SWAP_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            let _ = std::fs::remove_file(entry.path());
            continue;
        };
        let stamp = read_stamp(&entry.path());
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
                    // The file cannot be opened now; leave its swap where it is
                    // so a later start can try again.
                    Err(_) => continue,
                }
            }
        };
        // Two dead instances can leave a swap for the same file. Keep the newer
        // one instead of letting directory order decide.
        if recovered.contains(&id) {
            let adopted = std::fs::metadata(own.join(&name))
                .and_then(|meta| meta.modified())
                .ok();
            let candidate = metadata.modified().ok();
            let keep_existing = match (adopted, candidate) {
                (Some(adopted), Some(candidate)) => adopted >= candidate,
                _ => true,
            };
            if keep_existing {
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
        }
        let Some(document) = documents.get_mut(&id) else {
            continue;
        };
        document.restore(&text);
        if let Some((modified, len)) = stamp {
            document.record_disk_stamp(modified, len);
        }
        if !recovered.contains(&id) {
            recovered.push(id);
        }
        adopt(&entry.path(), own, &name);
    }
}

/// Move an adopted swap under `own` so this process prunes and clears it.
fn adopt(swap: &Path, own: &Path, name: &str) {
    if swap.parent() == Some(own) {
        return;
    }
    let target = own.join(name);
    let adopted = if std::fs::rename(swap, &target).is_ok() {
        true
    } else if std::fs::copy(swap, &target).is_ok() {
        let _ = std::fs::remove_file(swap);
        true
    } else {
        false
    };
    if !adopted {
        return;
    }
    let stamp = swap.with_extension("stamp");
    if stamp.exists() {
        let target = target.with_extension("stamp");
        if std::fs::rename(&stamp, &target).is_err() && std::fs::copy(&stamp, &target).is_ok() {
            let _ = std::fs::remove_file(&stamp);
        }
    }
}

/// Whether the instance that owns `dir` is gone, judged by taking its lock.
fn owner_gone(dir: &Path) -> bool {
    if is_live(dir) {
        return false;
    }
    if dir_pid(dir) == Some(std::process::id()) {
        // The name belongs to this process and no live instance claims it, so
        // the owner is a dropped instance even if a stray descriptor still holds
        // the lock.
        return true;
    }
    let Ok(lock) = File::options()
        .read(true)
        .write(true)
        .open(dir.join(LOCK_NAME))
    else {
        // No lock file means either the pre-directory layout or a directory
        // that is still being claimed; take it over only if it holds a swap.
        return has_swaps(dir);
    };
    match lock.try_lock() {
        Ok(()) => true,
        Err(std::fs::TryLockError::WouldBlock) => false,
        // Locking unavailable (descriptor pressure, unsupported filesystem):
        // apply the same rule as a missing lock file rather than leaving crash
        // data unreachable forever.
        Err(std::fs::TryLockError::Error(_)) => has_swaps(dir),
    }
}

fn has_swaps(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".swap"))
    })
}

/// True when the file on disk changed after the swap that holds it was written,
/// so saving the recovered buffer would overwrite newer content.
pub fn swap_is_stale(dir: &Path, path: &Path) -> bool {
    let swap = std::fs::metadata(dir.join(swap_name(path))).and_then(|meta| meta.modified());
    let file = std::fs::metadata(path).and_then(|meta| meta.modified());
    matches!((swap, file), (Ok(swap), Ok(file)) if file > swap)
}

/// Remove the swap for one path, after the user resolved it.
pub fn clear(dir: &Path, path: &Path) {
    let _ = std::fs::remove_file(dir.join(swap_name(path)));
    let _ = std::fs::remove_file(dir.join(stamp_name(path)));
}

fn swap_name(path: &Path) -> String {
    format!("{}.swap", hex(path.to_string_lossy().as_bytes()))
}

fn stamp_name(path: &Path) -> String {
    format!("{}.stamp", hex(path.to_string_lossy().as_bytes()))
}

/// Disk state the buffer was based on, stored next to the swap.
fn stamp_text(document: &Document) -> String {
    let stamp = document
        .disk_modified
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos().to_string())
        .unwrap_or_else(|| "-".to_string());
    format!("{stamp} {}", document.disk_len)
}

/// Read the stamp beside `swap`, if a previous build wrote one.
fn read_stamp(swap: &Path) -> Option<(Option<SystemTime>, u64)> {
    let text = std::fs::read_to_string(swap.with_extension("stamp")).ok()?;
    let (stamp, len) = text.trim().split_once(' ')?;
    let len = len.parse::<u64>().ok()?;
    let modified = if stamp == "-" {
        None
    } else {
        let nanos = u64::try_from(stamp.parse::<u128>().ok()?).ok()?;
        Some(UNIX_EPOCH + Duration::from_nanos(nanos))
    };
    Some((modified, len))
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

    fn swaps(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "swap"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_dirty_buffer_is_written_and_removed_after_save() {
        let base = dir("dirty");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(4, "x", 0, (0, 0));

        let crashed = Instance::claim(&base).expect("swap");
        write_to(crashed.dir(), [&document].into_iter()).expect("swap");
        assert_eq!(swaps(crashed.dir()).len(), 1);
        // A crash leaves the directory and releases its lock.
        drop(crashed);

        let instance = Instance::claim(&base).expect("swap");
        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        let recovered = recover_from(&base, instance.dir(), &mut documents, &mut ids);
        assert_eq!(recovered.len(), 1);
        let restored = documents.get_mut(&recovered[0]).expect("document");
        assert_eq!(restored.rope().to_string(), "one\nx");
        assert!(restored.dirty);
        assert_eq!(
            swaps(instance.dir()).len(),
            1,
            "adopted swap moves into own dir"
        );

        restored.save().expect("save");
        write_to(instance.dir(), documents.values()).expect("prune");
        assert!(swaps(instance.dir()).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_newer_of_two_crash_copies_wins() {
        let base = dir("duplicate");
        let file = base.join("note.txt");
        std::fs::write(&file, "disk\n").expect("seed");

        let older = Instance::claim(&base).expect("older");
        let newer = Instance::claim(&base).expect("newer");
        let own = Instance::claim(&base).expect("own");
        let name = swap_name(&file);
        std::fs::write(older.dir().join(&name), "old crash\n").expect("old swap");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(newer.dir().join(&name), "new crash\n").expect("new swap");
        drop(older);
        drop(newer);

        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        let recovered = recover_from(&base, own.dir(), &mut documents, &mut ids);
        assert_eq!(recovered.len(), 1, "one buffer per file, not per copy");
        let document = documents.get(&recovered[0]).expect("document");
        assert_eq!(document.rope().to_string(), "new crash\n");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_swap_without_its_file_is_dropped() {
        let base = dir("orphan");
        let file = base.join("gone.txt");
        std::fs::write(&file, "x\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(0, "y", 0, (0, 0));

        let crashed = Instance::claim(&base).expect("swap");
        write_to(crashed.dir(), [&document].into_iter()).expect("swap");
        drop(crashed);
        std::fs::remove_file(&file).expect("remove");

        let instance = Instance::claim(&base).expect("swap");
        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        assert!(recover_from(&base, instance.dir(), &mut documents, &mut ids).is_empty());
        assert!(swaps(instance.dir()).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_running_instance_keeps_its_swaps() {
        let base = dir("live");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file).expect("open");
        document.insert(4, "x", 0, (0, 0));

        let first = Instance::claim(&base).expect("first");
        let second = Instance::claim(&base).expect("second");
        write_to(first.dir(), [&document].into_iter()).expect("swap");
        write_to(second.dir(), std::iter::empty::<&Document>()).expect("prune");

        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        assert!(
            recover_from(&base, second.dir(), &mut documents, &mut ids).is_empty(),
            "a live instance's swaps are not adopted"
        );
        assert_eq!(swaps(first.dir()).len(), 1);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_graceful_exit_leaves_other_instances_alone() {
        let base = dir("exit");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file).expect("open");
        document.insert(4, "x", 0, (0, 0));

        let first = Instance::claim(&base).expect("first");
        let second = Instance::claim(&base).expect("second");
        write_to(first.dir(), [&document].into_iter()).expect("swap");
        write_to(second.dir(), std::iter::empty::<&Document>()).expect("prune");

        second.release();
        assert_eq!(swaps(first.dir()).len(), 1);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_stale_swap_is_reported() {
        let base = dir("stale");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(4, "x", 0, (0, 0));

        let instance = Instance::claim(&base).expect("swap");
        write_to(instance.dir(), [&document].into_iter()).expect("swap");
        assert!(!swap_is_stale(instance.dir(), &file));
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&file, "newer\n").expect("external write");
        assert!(swap_is_stale(instance.dir(), &file));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_swap_remembers_the_disk_state_it_was_based_on() {
        let base = dir("stamp");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(4, "x", 0, (0, 0));
        assert!(!document.disk_changed());

        let crashed = Instance::claim(&base).expect("swap");
        write_to(crashed.dir(), [&document].into_iter()).expect("swap");
        drop(crashed);
        // Another program writes the file after the swap was taken.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&file, "external\n").expect("external write");

        let instance = Instance::claim(&base).expect("swap");
        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        let recovered = recover_from(&base, instance.dir(), &mut documents, &mut ids);
        assert_eq!(recovered.len(), 1);
        let restored = &documents[&recovered[0]];
        assert_eq!(restored.rope().to_string(), "one\nx");
        assert!(
            restored.disk_changed(),
            "a swap must keep the stamp it was written against"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_swap_without_a_stamp_still_reads_as_text() {
        let base = dir("legacy");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let instance = Instance::claim(&base).expect("swap");
        // Pre-directory layout: the swap sits directly in the base directory.
        std::fs::write(base.join(swap_name(&file)), "legacy body\n").expect("swap");

        let mut documents = HashMap::new();
        let mut ids = Ids::default();
        let recovered = recover_from(&base, instance.dir(), &mut documents, &mut ids);
        assert_eq!(recovered.len(), 1);
        let restored = &documents[&recovered[0]];
        assert_eq!(restored.rope().to_string(), "legacy body\n");
        assert!(restored.dirty);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_swap_payload_is_plain_buffer_text() {
        let base = dir("plain");
        let file = base.join("note.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let mut document = Document::open(DocumentId(1), file.clone()).expect("open");
        document.insert(4, "x", 0, (0, 0));

        let instance = Instance::claim(&base).expect("swap");
        write_to(instance.dir(), [&document].into_iter()).expect("swap");
        let payload = std::fs::read_to_string(instance.dir().join(swap_name(&file))).expect("swap");
        assert_eq!(
            payload, "one\nx",
            "an older build must read the buffer as-is"
        );
        assert!(instance.dir().join(stamp_name(&file)).exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
