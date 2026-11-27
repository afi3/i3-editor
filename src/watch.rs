//! Filesystem watching.
//!
//! Watchers forward raw notify events to the event loop, which coalesces them
//! once per frame. Workspace watchers are recursive; when the OS refuses a
//! recursive watch they fall back to the expanded explorer directories plus
//! parent directories of open documents.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use notify::Watcher as _;
use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode};

pub struct Watcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    watched: HashSet<PathBuf>,
    recursive: bool,
    warning: Option<String>,
}

impl Watcher {
    /// Create a watcher with no paths; callers add paths with `watch`.
    pub fn empty() -> notify::Result<Self> {
        let (sender, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        Ok(Self {
            _watcher: watcher,
            events,
            watched: HashSet::new(),
            recursive: false,
            warning: None,
        })
    }

    /// Watch the workspace root recursively. When the OS refuses that, fall
    /// back to a non-recursive root watch and report it once through
    /// [`take_warning`](Self::take_warning). When `config` is given, its parent
    /// directory is watched non-recursively so atomic config saves are seen;
    /// these extra paths are not part of `sync_dirs`.
    pub fn new(root: &Path, config: Option<&Path>) -> notify::Result<Self> {
        let mut watcher = Self::empty()?;
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        match watcher._watcher.watch(&root, RecursiveMode::Recursive) {
            Ok(()) => {
                watcher.recursive = true;
                watcher.watched.insert(root.clone());
            }
            Err(error) => {
                watcher._watcher.watch(&root, RecursiveMode::NonRecursive)?;
                watcher.watched.insert(root.clone());
                watcher.warning = Some(format!(
                    "recursive watching failed for {}: {error}; \
                     falling back to expanded directories",
                    root.display()
                ));
            }
        }
        if let Some(parent) = config.and_then(Path::parent) {
            let parent = parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf());
            if parent != root {
                let _ = watcher._watcher.watch(&parent, RecursiveMode::NonRecursive);
                let plugins = parent.join("plugins");
                if plugins.is_dir() {
                    let _ = watcher
                        ._watcher
                        .watch(&plugins, RecursiveMode::NonRecursive);
                }
            }
        }
        Ok(watcher)
    }

    /// True when the workspace root is watched recursively.
    #[cfg(test)]
    pub fn is_recursive(&self) -> bool {
        self.recursive
    }

    /// The one-time fallback warning, if a recursive watch was refused.
    pub fn take_warning(&mut self) -> Option<String> {
        self.warning.take()
    }

    pub fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        self._watcher.watch(path, mode)?;
        self.watched.insert(path.to_path_buf());
        Ok(())
    }

    pub fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self._watcher.unwatch(path)?;
        self.watched.remove(path);
        Ok(())
    }

    /// Make the tracked directory set match `desired`, returning warnings for
    /// directories that could not be watched. Recursive watchers already cover
    /// every subdirectory, so this is a no-op for them.
    pub fn sync_dirs(&mut self, desired: &HashSet<PathBuf>) -> Vec<String> {
        if self.recursive {
            return Vec::new();
        }
        let stale: Vec<PathBuf> = self
            .watched
            .iter()
            .filter(|dir| !desired.contains(*dir))
            .cloned()
            .collect();
        for dir in stale {
            let _ = self._watcher.unwatch(&dir);
            self.watched.remove(&dir);
        }
        let mut warnings = Vec::new();
        for dir in desired {
            if self.watched.contains(dir) {
                continue;
            }
            match self._watcher.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(dir.clone());
                }
                Err(error) => warnings.push(format!("cannot watch {}: {error}", dir.display())),
            }
        }
        warnings
    }

    #[cfg(test)]
    pub fn watched_dirs(&self) -> &HashSet<PathBuf> {
        &self.watched
    }

    /// Next queued event without blocking, or `None` when the queue is empty.
    pub fn try_recv(&self) -> Option<notify::Result<Event>> {
        self.events.try_recv().ok()
    }
}

/// Whether an event can change explorer rows or an open buffer. Reads and other
/// access notifications do not.
pub fn relevant(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind};

    #[test]
    fn sync_dirs_adds_and_removes_tracked_directories() {
        let base = std::env::temp_dir().join(format!("i3-watch-sync-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let first = base.join("first");
        let second = base.join("second");
        std::fs::create_dir_all(&first).expect("first");
        std::fs::create_dir_all(&second).expect("second");

        let mut watcher = Watcher::empty().expect("watcher");
        let mut desired = HashSet::from([first.clone(), second.clone()]);
        assert!(watcher.sync_dirs(&desired).is_empty());
        assert!(watcher.watched_dirs().contains(&first));
        assert!(watcher.watched_dirs().contains(&second));

        desired.remove(&second);
        assert!(watcher.sync_dirs(&desired).is_empty());
        assert!(watcher.watched_dirs().contains(&first));
        assert!(!watcher.watched_dirs().contains(&second));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn workspace_watchers_are_recursive_and_skip_dir_sync() {
        let dir = std::env::temp_dir().join(format!("i3-watch-recursive-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).expect("nested");

        let mut watcher = Watcher::new(&dir, None).expect("watcher");
        assert!(watcher.is_recursive());
        assert!(watcher.take_warning().is_none());
        assert!(watcher.sync_dirs(&HashSet::from([nested])).is_empty());
        assert_eq!(watcher.watched_dirs().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn access_events_are_ignored() {
        assert!(!relevant(&EventKind::Access(AccessKind::Any)));
        assert!(relevant(&EventKind::Create(CreateKind::File)));
    }
}
