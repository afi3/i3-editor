//! Filesystem watcher integration: one coalesced pass per frame.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::state::WorkspaceId;
use crate::workspace::Workspace;

use super::{App, same_path};

/// Events drained per watcher per frame; a churning directory cannot starve
/// input handling.
const WATCH_BUDGET: usize = 4096;

impl App {
    /// Drain queued watcher events, refresh the explorer once, and react to
    /// open buffers whose files changed underneath them.
    pub(crate) fn pump_watcher(&mut self) {
        self.sync_workspace_watchers();
        self.sync_external_watcher();
        if self.watchers.is_empty() && self.external_watcher.is_none() {
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
        if let Some(watcher) = &self.external_watcher {
            paths.extend(drain_watcher(watcher));
        }
        if paths.is_empty() {
            return;
        }
        self.dirty = true;
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

    fn watcher_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.state.workspaces.active().id.hash(&mut hasher);
        self.state.explorer.revision.hash(&mut hasher);
        for workspace in &self.state.workspaces.workspaces {
            workspace.id.hash(&mut hasher);
            workspace.root.hash(&mut hasher);
        }
        let mut paths: Vec<&Path> = self
            .state
            .documents
            .values()
            .filter_map(|document| document.path.as_deref())
            .collect();
        paths.sort();
        for path in paths {
            path.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn desired_watch_dirs(&self, workspace: &Workspace, active: WorkspaceId) -> HashSet<PathBuf> {
        let root = workspace
            .root
            .canonicalize()
            .unwrap_or_else(|_| workspace.root.clone());
        let mut dirs = HashSet::from([root.clone()]);
        if workspace.id == active {
            dirs.extend(self.state.explorer.watched_dirs());
        }
        for path in self
            .state
            .documents
            .values()
            .filter_map(|document| document.path.as_deref())
        {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if path.starts_with(&root)
                && let Some(parent) = path.parent()
            {
                dirs.insert(parent.to_path_buf());
            }
        }
        dirs
    }

    /// Reconfigure non-recursive workspace watchers when the explorer expands,
    /// the active workspace changes, or an open document path changes.
    fn sync_workspace_watchers(&mut self) {
        let signature = self.watcher_signature();
        if self.watch_signature == Some(signature) {
            return;
        }
        self.watch_signature = Some(signature);
        let active = self.state.workspaces.active().id;
        let desired: HashMap<WorkspaceId, HashSet<PathBuf>> = self
            .state
            .workspaces
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, self.desired_watch_dirs(workspace, active)))
            .collect();
        let App {
            watchers, state, ..
        } = self;
        for (id, watcher) in watchers.iter_mut() {
            if let Some(dirs) = desired.get(id) {
                state.warnings.extend(watcher.sync_dirs(dirs));
            }
        }
    }

    /// Parent directories of open documents outside every workspace root.
    fn desired_external_dirs(&self) -> std::collections::HashSet<PathBuf> {
        let roots: Vec<PathBuf> = self
            .state
            .workspaces
            .workspaces
            .iter()
            .map(|workspace| {
                workspace
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| workspace.root.clone())
            })
            .collect();
        self.state
            .documents
            .values()
            .filter_map(|document| document.path.as_deref())
            .filter_map(|path| {
                let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                if roots.iter().any(|root| path.starts_with(root)) {
                    return None;
                }
                path.parent().map(Path::to_path_buf)
            })
            .collect()
    }

    /// Keep one non-recursive watcher over the parent directories of open
    /// documents that live outside every workspace root.
    fn sync_external_watcher(&mut self) {
        let desired = self.desired_external_dirs();
        if desired.is_empty() {
            self.external_watcher = None;
            self.external_watch_dirs.clear();
            return;
        }
        if self.external_watcher.is_none() {
            match crate::watch::Watcher::empty() {
                Ok(watcher) => self.external_watcher = Some(watcher),
                Err(error) => {
                    self.notify(format!(
                        "file watching disabled for external documents: {error}"
                    ));
                    return;
                }
            }
        }
        let stale: Vec<PathBuf> = self
            .external_watch_dirs
            .iter()
            .filter(|dir| !desired.contains(*dir))
            .cloned()
            .collect();
        for dir in stale {
            if let Some(watcher) = self.external_watcher.as_mut() {
                let _ = watcher.unwatch(&dir);
            }
            self.external_watch_dirs.remove(&dir);
        }
        let added: Vec<PathBuf> = desired
            .into_iter()
            .filter(|dir| !self.external_watch_dirs.contains(dir))
            .collect();
        for dir in added {
            let result = self
                .external_watcher
                .as_mut()
                .map(|watcher| watcher.watch(&dir, notify::RecursiveMode::NonRecursive));
            match result {
                Some(Ok(())) => {
                    self.external_watch_dirs.insert(dir);
                }
                Some(Err(error)) => {
                    self.notify(format!("cannot watch {}: {error}", dir.display()));
                }
                None => break,
            }
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
