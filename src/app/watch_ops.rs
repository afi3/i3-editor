//! Filesystem watcher integration: one coalesced pass per frame.

use std::path::Path;

use super::App;

impl App {
    /// Drain queued watcher events, refresh the explorer once, and react to
    /// open buffers whose files changed underneath them.
    pub(crate) fn pump_watcher(&mut self) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        let mut paths = Vec::new();
        while let Some(event) = watcher.try_recv() {
            let Ok(event) = event else {
                continue;
            };
            if crate::watch::relevant(&event.kind) {
                paths.extend(event.paths);
            }
        }
        if paths.is_empty() {
            return;
        }
        paths.sort();
        paths.dedup();

        // A `cargo build` churns `target/` constantly; only a visible path can
        // change explorer rows.
        if paths
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
