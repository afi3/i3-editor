//! Filesystem watching.
//!
//! One watcher follows the workspace root recursively and forwards raw notify
//! events to the event loop, which coalesces them once per frame.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use notify::Watcher as _;
use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode};

pub struct Watcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
}

impl Watcher {
    /// Watch the workspace root recursively. When `config` is given, its parent
    /// directory is watched non-recursively so atomic config saves are seen.
    pub fn new(root: &Path, config: Option<&Path>) -> notify::Result<Self> {
        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        if let Some(parent) = config.and_then(Path::parent) {
            let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
        }
        Ok(Self {
            _watcher: watcher,
            events,
        })
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
    fn access_events_are_ignored() {
        assert!(!relevant(&EventKind::Access(AccessKind::Any)));
        assert!(relevant(&EventKind::Create(CreateKind::File)));
    }
}
