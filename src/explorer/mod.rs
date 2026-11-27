//! File explorer state.
//!
//! Only expanded directories are read; rows are a flattened view of the
//! expansion state.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

pub struct ExplorerState {
    pub visible: bool,
    pub root: PathBuf,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub show_hidden: bool,
    expanded: HashSet<PathBuf>,
    ignore: Option<Gitignore>,
    /// Bumped on every rebuild; quick open caches its list against this.
    pub revision: u64,
}

impl ExplorerState {
    pub fn new(root: PathBuf, show_hidden: bool) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|error| Error::io("resolve", &root, error))?;
        let mut state = Self {
            visible: true,
            root: root.clone(),
            rows: Vec::new(),
            selected: 0,
            show_hidden,
            expanded: HashSet::new(),
            ignore: build_ignore(&root),
            revision: 0,
        };
        state.rebuild();
        Ok(state)
    }

    pub fn rebuild(&mut self) {
        let root = self.root.clone();
        let mut rows = Vec::new();
        self.walk(&root, 0, &mut rows);
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    fn walk(&self, dir: &Path, depth: usize, out: &mut Vec<Row>) {
        for path in self.read_dir(dir) {
            let is_dir = path.is_dir();
            let expanded = is_dir && self.expanded.contains(&path);
            out.push(Row {
                name: display_name(&path),
                path: path.clone(),
                depth,
                is_dir,
                expanded,
            });
            if expanded {
                self.walk(&path, depth + 1, out);
            }
        }
    }

    fn read_dir(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| self.is_visible(path))
            .collect();
        paths.sort_by_key(|path| (!path.is_dir(), display_name(path).to_lowercase()));
        paths
    }

    /// Whether the explorer would show `path`. File watching uses the same
    /// rules so ignored build output never rebuilds the tree.
    pub fn is_visible(&self, path: &Path) -> bool {
        // Showing hidden files also reveals entries hidden by `.gitignore`, so
        // a file such as `.env` becomes reachable with one toggle.
        if self.show_hidden {
            return true;
        }
        if display_name(path).starts_with('.') {
            return false;
        }
        match &self.ignore {
            Some(ignore) => !ignore
                .matched_path_or_any_parents(path, path.is_dir())
                .is_ignore(),
            None => true,
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last as isize) as usize;
    }
}

fn build_ignore(root: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    let _ = builder.add(root.join(".gitignore"));
    builder.build().ok()
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl ExplorerState {
    pub fn expand_selected(&mut self) {
        if let Some(row) = self.selected_row().cloned()
            && row.is_dir
            && self.expanded.insert(row.path.clone())
        {
            self.rebuild();
            self.reselect(&row.path);
        }
    }

    pub fn collapse_selected(&mut self) {
        if let Some(row) = self.selected_row().cloned() {
            if row.is_dir && row.expanded {
                self.expanded.remove(&row.path);
                self.rebuild();
                self.reselect(&row.path);
            } else if let Some(parent) = row.path.parent() {
                let parent = parent.to_path_buf();
                if self.expanded.remove(&parent) {
                    self.rebuild();
                    self.reselect(&parent);
                }
            }
        }
    }

    pub fn toggle_selected(&mut self) {
        match self.selected_row().cloned() {
            Some(row) if row.is_dir && row.expanded => self.collapse_selected(),
            Some(row) if row.is_dir => self.expand_selected(),
            _ => {}
        }
    }

    fn reselect(&mut self, path: &Path) {
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.selected = index;
        }
    }

    /// Directory a new entry belongs to: the selection when it is a directory,
    /// otherwise its parent.
    pub fn target_dir(&self) -> PathBuf {
        match self.selected_row() {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    pub fn create(&mut self, path: &Path, directory: bool) -> Result<PathBuf> {
        let path = path.to_path_buf();
        if path.file_name().is_none() {
            return Err(Error::InvalidPath(path));
        }
        if path.exists() {
            return Err(Error::AlreadyExists(path));
        }
        let result = if directory {
            std::fs::create_dir(&path)
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(|_| ())
        };
        result.map_err(|error| Error::io("create", &path, error))?;
        self.rebuild();
        self.reselect(&path);
        Ok(path)
    }

    pub fn rename(&mut self, new_name: &str) -> Result<PathBuf> {
        let row = self
            .selected_row()
            .cloned()
            .ok_or_else(|| Error::InvalidPath(PathBuf::new()))?;
        let name = validate_name(new_name)?;
        let parent = row
            .path
            .parent()
            .ok_or_else(|| Error::InvalidPath(row.path.clone()))?;
        let target = parent.join(name);
        if target.exists() {
            return Err(Error::AlreadyExists(target));
        }
        std::fs::rename(&row.path, &target)
            .map_err(|error| Error::io("rename", &row.path, error))?;
        self.forget_subtree(&row.path);
        self.rebuild();
        self.reselect(&target);
        Ok(target)
    }

    /// Copy `source` beside itself, adding ` copy`, then ` copy 2`, ... until
    /// the name is free. Only regular files are duplicated.
    pub fn duplicate(&mut self, source: &Path) -> Result<PathBuf> {
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let source = resolve_in_root(&root, source)?;
        if !source.is_file() {
            return Err(Error::InvalidPath(source));
        }
        let parent = source
            .parent()
            .and_then(|parent| parent.strip_prefix(&root).ok())
            .ok_or_else(|| Error::InvalidPath(source.clone()))?;
        let stem = source
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "file".to_string());
        let extension = source
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy()))
            .unwrap_or_default();
        let mut target = root.join(parent).join(format!("{stem} copy{extension}"));
        let mut index = 2;
        while target.exists() {
            target = root
                .join(parent)
                .join(format!("{stem} copy {index}{extension}"));
            index += 1;
        }
        if !target.starts_with(&root) {
            return Err(Error::InvalidPath(target));
        }
        std::fs::copy(&source, &target).map_err(|error| Error::io("duplicate", &source, error))?;
        self.rebuild();
        self.reselect(&target);
        Ok(target)
    }

    pub fn delete_at(&mut self, path: &Path) -> Result<PathBuf> {
        let path = path.to_path_buf();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else if path.exists() {
            std::fs::remove_file(&path)
        } else {
            return Err(Error::InvalidPath(path));
        };
        result.map_err(|error| Error::io("delete", &path, error))?;
        self.forget_subtree(&path);
        self.rebuild();
        Ok(path)
    }

    fn forget_subtree(&mut self, path: &Path) {
        self.expanded.retain(|expanded| !expanded.starts_with(path));
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild();
    }

    pub fn refresh(&mut self) {
        self.ignore = build_ignore(&self.root);
        self.rebuild();
    }

    /// Root plus every expanded directory; the non-recursive fallback follows these.
    pub fn watched_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::with_capacity(self.expanded.len() + 1);
        dirs.push(self.root.clone());
        dirs.extend(self.expanded.iter().cloned());
        dirs
    }
}

fn resolve_in_root(root: &Path, path: &Path) -> Result<PathBuf> {
    let resolved = path
        .canonicalize()
        .map_err(|error| Error::io("resolve", path, error))?;
    if !resolved.starts_with(root) {
        return Err(Error::InvalidPath(resolved));
    }
    Ok(resolved)
}

fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(Error::InvalidPath(PathBuf::from(name)));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_hidden_reveals_dotfiles_and_gitignored_entries() {
        let dir = std::env::temp_dir().join(format!("i3-explorer-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("target")).expect("target dir");
        std::fs::write(dir.join(".gitignore"), ".env\ntarget/\n").expect("gitignore");
        std::fs::write(dir.join(".env"), "SECRET=1\n").expect("dotfile");
        std::fs::write(dir.join("target").join("artifact"), "").expect("ignored file");
        std::fs::write(dir.join("main.rs"), "").expect("file");

        let mut explorer = ExplorerState::new(dir.clone(), false).expect("explorer");
        let visible: Vec<&str> = explorer.rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(visible, ["main.rs"]);

        explorer.toggle_hidden();
        let visible: Vec<&str> = explorer.rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(visible, ["target", ".env", ".gitignore", "main.rs"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn watched_dirs_follow_expanded_directories() {
        let dir = std::env::temp_dir().join(format!("i3-explorer-watch-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("sub")).expect("sub");
        let mut explorer = ExplorerState::new(dir.clone(), false).expect("explorer");
        assert_eq!(
            explorer.watched_dirs(),
            vec![dir.canonicalize().expect("canonical")]
        );
        let index = explorer
            .rows
            .iter()
            .position(|row| row.name == "sub")
            .expect("sub row");
        explorer.selected = index;
        explorer.expand_selected();
        assert!(
            explorer
                .watched_dirs()
                .contains(&dir.join("sub").canonicalize().expect("canonical"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn duplicate_rejects_paths_outside_the_root() {
        let dir = std::env::temp_dir().join(format!("i3-explorer-dup-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("i3-explorer-dup-out-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&outside).ok();
        std::fs::create_dir_all(&dir).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        let source = outside.join("secret.txt");
        std::fs::write(&source, "secret\n").expect("source");
        let mut explorer = ExplorerState::new(dir.clone(), false).expect("explorer");

        assert!(explorer.duplicate(&source).is_err());
        assert!(!dir.join("secret copy.txt").exists());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}
