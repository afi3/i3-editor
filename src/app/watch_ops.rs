//! Filesystem watcher integration: one coalesced pass per frame.

use std::path::{Path, PathBuf};

use super::App;

/// Events drained per watcher per frame; a churning directory cannot starve
/// input handling.
const WATCH_BUDGET: usize = 4096;

impl App {
    /// Drain queued watcher events, refresh the explorer once, and react to
    /// open buffers whose files changed underneath them.
    pub(crate) fn pump_watcher(&mut self) {
        if self.watchers.is_empty() {
            return;
        }
        let active = self.state.workspaces.active().id;
        let mut paths = Vec::new();
        let mut active_events = false;
        for (id, watcher) in &self.watchers {
            let events = drain_watcher(watcher);
            if !events.is_empty() && *id == active {
                active_events = true;
            }
            paths.extend(events);
        }
        if paths.is_empty() {
            return;
        }
        let config_path = self.state.config_path.clone();
        let config_dir = config_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(std::path::Path::to_path_buf);
        let config_changed = config_path
            .as_ref()
            .is_some_and(|config| paths.iter().any(|path| same_path(path, config)));
        if config_changed {
            self.reload_config();
        }
        if let Some(dir) = config_dir {
            paths.retain(|path| path.parent() != Some(dir.as_path()));
        }
        if let Some(plugins) = crate::plugins::Plugins::default_dir() {
            let changed = paths.iter().any(|path| path.starts_with(&plugins));
            if changed {
                self.reload_plugins();
                paths.retain(|path| !path.starts_with(&plugins));
            }
        }
        if paths.is_empty() {
            return;
        }
        paths.sort();
        paths.dedup();

        // A `cargo build` churns `target/` constantly; only a visible path can
        // change explorer rows, and only the active workspace owns the tree.
        if active_events
            && paths
                .iter()
                .any(|path| self.state.explorer.is_visible(path))
        {
            self.state.explorer.refresh();
        }
        for path in paths {
            self.handle_external_change(&path);
        }
    }

    pub(crate) fn handle_external_change(&mut self, path: &Path) {
        let Some(id) = self
            .state
            .documents
            .values()
            .find(|document| {
                document
                    .path
                    .as_deref()
                    .is_some_and(|open| same_path(open, path))
            })
            .map(|document| document.id)
        else {
            return;
        };
        let document = &self.state.documents[&id];
        if !document.disk_changed() {
            return;
        }
        let name = document.display_name();
        let dirty = document.dirty;
        if dirty {
            self.notify(format!(
                "{name} changed on disk; save to overwrite or reload"
            ));
            return;
        }
        if !path.exists() {
            self.notify(format!("{name} deleted on disk"));
            return;
        }
        match self.reload_document(id) {
            Ok(()) => self.notify(format!("{name} reloaded from disk")),
            Err(error) => self.notify(error.to_string()),
        }
    }
}

fn drain_watcher(watcher: &crate::watch::Watcher) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for _ in 0..WATCH_BUDGET {
        let Some(event) = watcher.try_recv() else {
            break;
        };
        let Ok(event) = event else {
            continue;
        };
        if crate::watch::relevant(&event.kind) {
            paths.extend(event.paths);
        }
    }
    paths
}

/// Watcher paths and document paths can differ by symlinked prefixes on macOS
/// (`/var` vs `/private/var`), so fall back to comparing resolved paths.
fn same_path(open: &Path, event: &Path) -> bool {
    if open == event {
        return true;
    }
    match (open.canonicalize(), event.canonicalize()) {
        (Ok(open), Ok(event)) => open == event,
        _ => false,
    }
}
