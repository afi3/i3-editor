//! Session persistence: workspaces, layouts, and open files.
//!
//! Saved when the editor exits and restored when it starts without a path
//! argument in the same directory. Only files on disk are persisted: an untitled
//! buffer has no identity to restore, and a discarded dirty buffer comes back
//! with its on-disk content.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::editor::document::Document;
use crate::state::{AppState, DocumentId, Focus, Pane, PaneId};
use crate::workspace::layout::{LayoutNode, MAX_CHILDREN, MAX_TABS, MAX_WEIGHT, SplitMode};
use crate::workspace::{SplitAxis, Workspace};

const VERSION: u32 = 1;
const MAX_WORKSPACES: usize = 32;
const MAX_PANES: usize = 64;
const MAX_DOCUMENTS: usize = 512;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Serialize, Deserialize)]
struct Session {
    version: u32,
    root: PathBuf,
    active: usize,
    show_hidden: bool,
    documents: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    folds: Vec<Vec<(usize, usize)>>,
    workspaces: Vec<WorkspaceSession>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceSession {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_dir: Option<PathBuf>,
    focus: usize,
    panes: Vec<PaneSession>,
    layout: Node,
}

#[derive(Debug, Serialize, Deserialize)]
struct PaneSession {
    cursor: usize,
    scroll_top: usize,
    scroll_left: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<usize>,
    tabs: Vec<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Node {
    Pane {
        index: usize,
    },
    Split {
        axis: Axis,
        mode: Mode,
        active: usize,
        weights: Vec<u16>,
        children: Vec<Node>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Axis {
    LeftRight,
    TopBottom,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Tiled,
    Tabbed,
}

impl From<SplitAxis> for Axis {
    fn from(axis: SplitAxis) -> Self {
        match axis {
            SplitAxis::LeftRight => Axis::LeftRight,
            SplitAxis::TopBottom => Axis::TopBottom,
        }
    }
}

impl From<Axis> for SplitAxis {
    fn from(axis: Axis) -> Self {
        match axis {
            Axis::LeftRight => SplitAxis::LeftRight,
            Axis::TopBottom => SplitAxis::TopBottom,
        }
    }
}

impl From<SplitMode> for Mode {
    fn from(mode: SplitMode) -> Self {
        match mode {
            SplitMode::Tiled => Mode::Tiled,
            SplitMode::Tabbed => Mode::Tabbed,
        }
    }
}

impl From<Mode> for SplitMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Tiled => SplitMode::Tiled,
            Mode::Tabbed => SplitMode::Tabbed,
        }
    }
}

pub fn default_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|dir| dir.join("session.toml"))
}

/// Session file for one workspace root. Two editors in different projects then
/// keep their own layout instead of overwriting a single shared file. Keyed by
/// the canonical root so a symlinked path lands on the same file.
pub fn path_for(root: &Path) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let tag: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::config::config_dir().map(|dir| dir.join(format!("session-{tag}.toml")))
}

/// Restore the session for this directory. Warnings are returned for the status
/// bar; a missing or foreign session is not an error.
pub fn restore(state: &mut AppState) -> Vec<String> {
    let path = match path_for(&state.explorer.root) {
        Some(path) if path.exists() => path,
        // Older builds kept one file for every root; read it until the next
        // save writes the per-root file.
        _ => match default_path() {
            Some(path) => path,
            None => return Vec::new(),
        },
    };
    match restore_from(&path, state) {
        Ok(warnings) => warnings,
        Err(warning) => vec![warning],
    }
}

/// Save `state` to `path`. An empty session (no pane with a file) writes nothing.
pub fn save_to(path: &Path, state: &AppState) -> std::io::Result<()> {
    save_to_root(path, state, &state.explorer.root)
}

/// Save `state` stamped with `root`, so the per-root file of a workspace that
/// is not active still restores its own layout.
pub fn save_to_root(path: &Path, state: &AppState, root: &Path) -> std::io::Result<()> {
    let Some(session) = snapshot_for(state, root.to_path_buf()) else {
        // Nothing worth restoring: drop a session from an earlier run so it
        // cannot resurrect files the user has closed.
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
    };
    let text = toml::to_string(&session).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    crate::editor::document::write_atomic(path, text.as_bytes())
}

fn snapshot_for(state: &AppState, root: PathBuf) -> Option<Session> {
    let mut documents: Vec<PathBuf> = Vec::new();
    let mut folds: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut positions: HashMap<PathBuf, usize> = HashMap::new();
    let mut workspaces = Vec::new();
    let mut active = 0;

    for (index, workspace) in state.workspaces.workspaces.iter().enumerate() {
        let mut pane_indices: HashMap<PaneId, usize> = HashMap::new();
        let mut panes = Vec::new();
        for pane_id in workspace.layout.leaves() {
            let Some(pane) = state.panes.get(&pane_id) else {
                continue;
            };
            let mut tabs = Vec::new();
            let mut active_tab = None;
            for document_id in &pane.tabs {
                let Some(document) = state.documents.get(document_id) else {
                    continue;
                };
                let Some(path) = &document.path else {
                    continue;
                };
                let position = match positions.get(path) {
                    Some(position) => *position,
                    None => {
                        let position = documents.len();
                        documents.push(path.clone());
                        folds.push(
                            document
                                .folded
                                .iter()
                                .map(|range| (range.start, range.end))
                                .collect(),
                        );
                        positions.insert(path.clone(), position);
                        position
                    }
                };
                tabs.push(position);
                if pane.active == Some(*document_id) {
                    active_tab = Some(tabs.len() - 1);
                }
            }
            if active_tab.is_none() {
                active_tab = tabs.first().copied();
            }
            pane_indices.insert(pane_id, panes.len());
            panes.push(PaneSession {
                cursor: pane.cursor,
                scroll_top: pane.scroll_top,
                scroll_left: pane.scroll_left,
                active: active_tab,
                tabs,
            });
        }
        if panes.is_empty() {
            continue;
        }
        let Some(layout) = snapshot_node(&workspace.layout, &pane_indices) else {
            continue;
        };
        if state.workspaces.active == index {
            active = workspaces.len();
        }
        workspaces.push(WorkspaceSession {
            name: workspace.name.clone(),
            root: Some(workspace.root.clone()),
            terminal_dir: Some(workspace.terminal_dir.clone()),
            focus: pane_indices.get(&workspace.focus).copied().unwrap_or(0),
            panes,
            layout,
        });
    }

    if workspaces.is_empty() || documents.is_empty() {
        return None;
    }
    // A file written for another root must start on that root's workspace,
    // not on whichever workspace happened to be active during the save.
    let active = workspaces
        .iter()
        .position(|workspace| workspace.root.as_deref() == Some(root.as_path()))
        .unwrap_or(active.min(workspaces.len() - 1));
    Some(Session {
        version: VERSION,
        root,
        active,
        show_hidden: state.explorer.show_hidden,
        documents,
        folds,
        workspaces,
    })
}

fn snapshot_node(node: &LayoutNode, panes: &HashMap<PaneId, usize>) -> Option<Node> {
    match node {
        LayoutNode::Pane(id) => Some(Node::Pane {
            index: *panes.get(id)?,
        }),
        LayoutNode::Split {
            axis,
            mode,
            active,
            weights,
            children,
        } => {
            let children = children
                .iter()
                .map(|child| snapshot_node(child, panes))
                .collect::<Option<Vec<_>>>()?;
            Some(Node::Split {
                axis: (*axis).into(),
                mode: (*mode).into(),
                active: *active,
                weights: weights.clone(),
                children,
            })
        }
    }
}

/// Restore `state` from `path`; returns warnings for skipped files.
pub fn restore_from(path: &Path, state: &mut AppState) -> std::result::Result<Vec<String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("session not read: {error}")),
    };
    let session: Session = match toml::from_str(&text) {
        Ok(session) => session,
        Err(error) => return Ok(vec![format!("session ignored: {error}")]),
    };
    if session.root != state.explorer.root {
        return Ok(Vec::new());
    }
    if let Err(reason) = validate(&session) {
        return Ok(vec![format!("session ignored: {reason}")]);
    }

    state.documents.clear();
    state.panes.clear();
    state.workspaces.workspaces.clear();

    let (warnings, documents) = restore_documents(&session, state);
    restore_workspaces(&session, state, &documents);

    state.workspaces.active = session.active;
    state.focus = Focus::Pane(state.workspaces.active().focus);
    state.fullscreen = None;
    state.moving = false;
    state.search = None;
    state.explorer.show_hidden = session.show_hidden;
    state.explorer.refresh();
    Ok(warnings)
}

fn restore_documents(
    session: &Session,
    state: &mut AppState,
) -> (Vec<String>, Vec<Option<DocumentId>>) {
    let mut warnings = Vec::new();
    let mut documents: Vec<Option<DocumentId>> = Vec::with_capacity(session.documents.len());
    let mut fold_sets = session.folds.iter();
    for path in &session.documents {
        let folds = fold_sets.next();
        let id: DocumentId = state.ids.document();
        let opened = Document::open(id, path.clone()).or_else(|error| {
            if matches!(
                error,
                crate::error::Error::FileTooLarge { size, .. }
                    if size <= crate::editor::document::MAX_READ_ONLY_BYTES
            ) {
                Document::open_read_only(id, path.clone())
            } else {
                Err(error)
            }
        });
        match opened {
            Ok(mut document) => {
                if let Some(ranges) = folds {
                    document.restore_folds(ranges.iter().map(|(start, end)| *start..*end));
                }
                state.documents.insert(id, document);
                documents.push(Some(id));
            }
            Err(error) => {
                warnings.push(format!("session: skipped {}: {error}", path.display()));
                documents.push(None);
            }
        }
    }
    (warnings, documents)
}

fn restore_workspaces(session: &Session, state: &mut AppState, documents: &[Option<DocumentId>]) {
    for workspace in &session.workspaces {
        let mut pane_ids = Vec::new();
        for pane in &workspace.panes {
            let id = state.ids.pane();
            let mut restored = Pane::new(id);
            for tab in &pane.tabs {
                if let Some(Some(document)) = documents.get(*tab) {
                    restored.tabs.push(*document);
                }
            }
            restored.active = pane
                .active
                .and_then(|index| pane.tabs.get(index))
                .and_then(|index| documents.get(*index).copied().flatten())
                .or_else(|| restored.tabs.first().copied());
            if let Some(document) = restored.active.and_then(|id| state.documents.get(&id)) {
                restored.cursor = pane.cursor.min(document.len_chars());
                restored.scroll_top = pane.scroll_top.min(document.line_count().saturating_sub(1));
                restored.scroll_row = 0;
            } else {
                restored.scroll_top = 0;
                restored.scroll_row = 0;
            }
            restored.scroll_left = pane.scroll_left;
            state.panes.insert(id, restored);
            pane_ids.push(id);
        }
        let layout = build_node(&workspace.layout, &pane_ids);
        let focus = pane_ids
            .get(workspace.focus)
            .copied()
            .unwrap_or(pane_ids[0]);
        let id = state.ids.workspace();
        let root = workspace_root(workspace, &session.root);
        let terminal_dir = workspace
            .terminal_dir
            .clone()
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(|| root.clone());
        state.workspaces.workspaces.push(Workspace {
            id,
            name: workspace.name.clone(),
            layout,
            focus,
            terminal_dir,
            root,
        });
    }
}

fn workspace_root(workspace: &WorkspaceSession, fallback: &Path) -> PathBuf {
    workspace
        .root
        .clone()
        .filter(|root| root.is_dir())
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn build_node(node: &Node, panes: &[PaneId]) -> LayoutNode {
    match node {
        Node::Pane { index } => LayoutNode::Pane(panes.get(*index).copied().unwrap_or(panes[0])),
        Node::Split {
            axis,
            mode,
            active,
            weights,
            children,
        } => LayoutNode::Split {
            axis: (*axis).into(),
            mode: (*mode).into(),
            active: *active,
            weights: weights.clone(),
            children: children
                .iter()
                .map(|child| build_node(child, panes))
                .collect(),
        },
    }
}

fn validate(session: &Session) -> std::result::Result<(), String> {
    if session.version != VERSION {
        return Err(format!(
            "unsupported version {} (expected {VERSION})",
            session.version
        ));
    }
    if session.workspaces.is_empty() {
        return Err("no workspaces".to_string());
    }
    if session.workspaces.len() > MAX_WORKSPACES {
        return Err("too many workspaces".to_string());
    }
    if session.documents.len() > MAX_DOCUMENTS {
        return Err("too many documents".to_string());
    }
    if session.active >= session.workspaces.len() {
        return Err("active workspace out of range".to_string());
    }
    for workspace in &session.workspaces {
        if workspace.panes.is_empty() {
            return Err(format!("workspace {:?} has no panes", workspace.name));
        }
        if workspace.panes.len() > MAX_PANES {
            return Err(format!("workspace {:?} has too many panes", workspace.name));
        }
        if workspace.focus >= workspace.panes.len() {
            return Err(format!("workspace {:?} focus out of range", workspace.name));
        }
        for pane in &workspace.panes {
            if pane.tabs.len() > MAX_TABS {
                return Err("pane has too many tabs".to_string());
            }
            if pane.active.is_some_and(|active| active >= pane.tabs.len()) {
                return Err("pane active tab out of range".to_string());
            }
            for tab in &pane.tabs {
                if *tab >= session.documents.len() {
                    return Err("pane tab references a missing document".to_string());
                }
            }
        }
        let mut seen = vec![false; workspace.panes.len()];
        validate_node(&workspace.layout, workspace.panes.len(), &mut seen, 0)?;
        if seen.iter().any(|seen| !seen) {
            return Err("layout does not include every pane".to_string());
        }
    }
    Ok(())
}

fn validate_node(
    node: &Node,
    panes: usize,
    seen: &mut [bool],
    depth: usize,
) -> std::result::Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("layout is too deep".to_string());
    }
    match node {
        Node::Pane { index } => {
            if *index >= panes {
                return Err("layout pane index out of range".to_string());
            }
            if seen[*index] {
                return Err("layout repeats a pane".to_string());
            }
            seen[*index] = true;
        }
        Node::Split {
            mode,
            active,
            weights,
            children,
            ..
        } => {
            // A tabbed container holds tabs, not panes; `MAX_TABS` is its limit.
            let limit = match mode {
                Mode::Tabbed => MAX_TABS,
                Mode::Tiled => MAX_CHILDREN,
            };
            if children.len() < 2 || children.len() > limit {
                return Err("split has an invalid child count".to_string());
            }
            if weights.len() != children.len() {
                return Err("split weights do not match its children".to_string());
            }
            if weights
                .iter()
                .any(|weight| *weight == 0 || *weight > MAX_WEIGHT)
            {
                return Err("split weight out of range".to_string());
            }
            if *active >= children.len() {
                return Err("split active child out of range".to_string());
            }
            for child in children {
                validate_node(child, panes, seen, depth + 1)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::explorer::ExplorerState;
    use crate::input::KeyMap;
    use crate::state::Ids;
    use crate::theme::Theme;
    use crate::ui::UiState;
    use crate::workspace::WorkspaceManager;

    #[test]
    fn a_tabbed_split_may_hold_more_than_four_tabs() {
        let children: Vec<Node> = (0..5).map(|index| Node::Pane { index }).collect();
        let node = Node::Split {
            axis: Axis::LeftRight,
            mode: Mode::Tabbed,
            active: 0,
            weights: vec![1; 5],
            children,
        };
        let mut seen = vec![false; 5];
        assert!(
            validate_node(&node, 5, &mut seen, 0).is_ok(),
            "a five-tab container must survive a save/restore"
        );

        let tiled = Node::Split {
            axis: Axis::LeftRight,
            mode: Mode::Tiled,
            active: 0,
            weights: vec![1; 5],
            children: (0..5).map(|index| Node::Pane { index }).collect(),
        };
        let mut seen = vec![false; 5];
        assert!(
            validate_node(&tiled, 5, &mut seen, 0).is_err(),
            "a tiled split still caps at MAX_CHILDREN"
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("i3-session-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn state_with(root: &Path) -> AppState {
        let mut ids = Ids::default();
        let pane_id = ids.pane();
        let workspace_id = ids.workspace();
        let mut panes = HashMap::new();
        panes.insert(pane_id, Pane::new(pane_id));
        let mut workspaces = WorkspaceManager::default();
        workspaces.push(Workspace::new(
            workspace_id,
            "main",
            pane_id,
            root.to_path_buf(),
        ));
        AppState {
            should_quit: false,
            explorer: ExplorerState::new(root.to_path_buf(), false).expect("explorer"),
            documents: HashMap::new(),
            panes,
            workspaces,
            focus: Focus::Pane(pane_id),
            mode: crate::state::InputMode::Normal,
            keymap: KeyMap::default(),
            ui: UiState::default(),
            config: AppConfig::default(),
            theme: Theme::default(),
            ids,
            warnings: Vec::new(),
            clipboard: String::new(),
            search: None,
            search_options: crate::search_options::Options::default(),
            project_search: None,
            lsp_diagnostics: HashMap::new(),
            completions: None,
            hover: None,
            git_marks: HashMap::new(),
            git_diff: None,
            help: None,
            mcp_view: None,
            rules_view: None,
            trace_view: None,
            ai: crate::state::AiPanel::default(),
            hunks: None,
            plugin_status: Vec::new(),
            fullscreen: None,
            moving: false,
            terminals: Default::default(),
            themes_dir: PathBuf::new(),
            config_path: None,
        }
    }

    fn add_document(state: &mut AppState, path: &Path) -> DocumentId {
        let id = state.ids.document();
        let document = Document::open(id, path.to_path_buf()).expect("open");
        state.documents.insert(id, document);
        id
    }

    fn first_pane_id(state: &AppState) -> PaneId {
        state.workspaces.active().layout.leaves()[0]
    }

    #[test]
    fn session_round_trip_restores_layout_tabs_and_cursor() {
        let dir = temp_dir("round");
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        std::fs::write(&a, "fn a() {\n    one();\n}\n").expect("a");
        std::fs::write(&b, "fn b() {}\n\n").expect("b");

        let mut state = state_with(&dir);
        let first = first_pane_id(&state);
        let a_id = add_document(&mut state, &a);
        let b_id = add_document(&mut state, &b);
        state
            .documents
            .get_mut(&a_id)
            .expect("a")
            .restore_folds(std::iter::once(0..2));
        state.panes.get_mut(&first).expect("pane").tabs.push(a_id);
        state.panes.get_mut(&first).expect("pane").active = Some(a_id);
        state.panes.get_mut(&first).expect("pane").cursor = 3;
        state.panes.get_mut(&first).expect("pane").scroll_top = 1;

        let second = state.ids.pane();
        let mut pane = Pane::new(second);
        pane.tabs.push(b_id);
        pane.active = Some(b_id);
        state.panes.insert(second, pane);
        state
            .workspaces
            .active_mut()
            .layout
            .split_pane(first, SplitAxis::LeftRight, second);

        let session_path = dir.join("session.toml");
        let terminal_dir = dir.join("terminal");
        std::fs::create_dir_all(&terminal_dir).expect("terminal dir");
        state.workspaces.active_mut().terminal_dir = terminal_dir.clone();
        save_to(&session_path, &state).expect("save session");

        let mut restored = state_with(&dir);
        let warnings = restore_from(&session_path, &mut restored).expect("restore");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(restored.documents.len(), 2);
        assert_eq!(restored.panes.len(), 2);
        assert_eq!(restored.workspaces.active().name, "main");
        assert_eq!(restored.workspaces.active().terminal_dir, terminal_dir);
        assert_eq!(restored.workspaces.active().layout.leaves().len(), 2);

        let pane = restored.panes.get(&first_pane_id(&restored)).expect("pane");
        assert_eq!(pane.cursor, 3);
        assert_eq!(pane.scroll_top, 1);
        let active = pane.active.expect("active");
        assert_eq!(
            restored.documents[&active].path.as_deref(),
            Some(a.as_path())
        );
        let restored_a = restored
            .documents
            .values()
            .find(|document| document.path.as_deref() == Some(a.as_path()))
            .expect("restored a");
        assert_eq!(restored_a.folded, vec![0..2]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_files_are_skipped_with_a_warning() {
        let dir = temp_dir("missing");
        let a = dir.join("a.rs");
        std::fs::write(&a, "a\n").expect("a");

        let mut state = state_with(&dir);
        let first = first_pane_id(&state);
        let a_id = add_document(&mut state, &a);
        state.panes.get_mut(&first).expect("pane").tabs.push(a_id);
        state.panes.get_mut(&first).expect("pane").active = Some(a_id);

        let session_path = dir.join("session.toml");
        save_to(&session_path, &state).expect("save session");
        std::fs::remove_file(&a).expect("remove");

        let mut restored = state_with(&dir);
        let warnings = restore_from(&session_path, &mut restored).expect("restore");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("a.rs"), "{warnings:?}");
        assert!(restored.documents.is_empty());
        assert!(
            restored
                .panes
                .values()
                .all(|pane| pane.tabs.is_empty() && pane.active.is_none())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_or_foreign_session_is_ignored() {
        let dir = temp_dir("broken");
        let session_path = dir.join("session.toml");
        let mut state = state_with(&dir);
        let panes_before = state.panes.len();

        std::fs::write(&session_path, "not toml").expect("write");
        let warnings = restore_from(&session_path, &mut state).expect("restore");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(state.panes.len(), panes_before);

        let foreign = Session {
            version: VERSION,
            root: dir.join("elsewhere"),
            active: 0,
            show_hidden: false,
            documents: Vec::new(),
            folds: Vec::new(),
            workspaces: Vec::new(),
        };
        std::fs::write(&session_path, toml::to_string(&foreign).expect("toml")).expect("write");
        let warnings = restore_from(&session_path, &mut state).expect("restore");
        assert!(warnings.is_empty(), "foreign roots are skipped silently");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn untitled_buffers_are_not_persisted() {
        let dir = temp_dir("untitled");
        let mut state = state_with(&dir);
        let first = first_pane_id(&state);
        let id = state.ids.document();
        state.documents.insert(id, Document::empty(id, None));
        state.panes.get_mut(&first).expect("pane").tabs.push(id);
        state.panes.get_mut(&first).expect("pane").active = Some(id);

        let session_path = dir.join("session.toml");
        std::fs::write(&session_path, "stale").expect("stale session");
        save_to(&session_path, &state).expect("save");
        assert!(!session_path.exists(), "stale session must be removed");

        let mut restored = state_with(&dir);
        restore_from(&session_path, &mut restored).expect("restore");
        assert!(restored.documents.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_workspace_root_can_be_stamped_into_its_own_session_file() {
        let dir = temp_dir("stamped");
        let other = dir.join("other");
        std::fs::create_dir_all(&other).expect("other");
        let a = dir.join("a.rs");
        std::fs::write(&a, "fn a() {}\n").expect("a");

        let mut state = state_with(&dir);
        let first = first_pane_id(&state);
        let a_id = add_document(&mut state, &a);
        state.panes.get_mut(&first).expect("pane").tabs.push(a_id);
        state.panes.get_mut(&first).expect("pane").active = Some(a_id);

        let second = state.ids.pane();
        state.panes.insert(second, Pane::new(second));
        let workspace = state.ids.workspace();
        state
            .workspaces
            .workspaces
            .push(Workspace::new(workspace, "other", second, other.clone()));
        state.workspaces.active = 0;

        let path = dir.join("session-other.toml");
        save_to_root(&path, &state, &other).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        let session: Session = toml::from_str(&text).expect("toml");
        assert_eq!(session.root, other);
        assert_eq!(
            session.active, 1,
            "the file starts on the workspace that owns its root"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workspace_roots_round_trip() {
        let dir = temp_dir("roots");
        let other = dir.join("other");
        std::fs::create_dir_all(&other).expect("other");
        let a = dir.join("a.rs");
        std::fs::write(&a, "fn a() {}\n").expect("a");

        let mut state = state_with(&dir);
        let first = first_pane_id(&state);
        let a_id = add_document(&mut state, &a);
        state.panes.get_mut(&first).expect("pane").tabs.push(a_id);
        state.panes.get_mut(&first).expect("pane").active = Some(a_id);

        let second = state.ids.pane();
        state.panes.insert(second, Pane::new(second));
        let workspace = state.ids.workspace();
        state
            .workspaces
            .workspaces
            .push(Workspace::new(workspace, "other", second, other.clone()));

        let session_path = dir.join("session.toml");
        save_to(&session_path, &state).expect("save session");

        let mut restored = state_with(&dir);
        restore_from(&session_path, &mut restored).expect("restore");
        assert_eq!(restored.workspaces.workspaces.len(), 2);
        assert_eq!(restored.workspaces.workspaces[1].root, other);
        std::fs::remove_dir_all(&dir).ok();
    }
}
