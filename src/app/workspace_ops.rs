//! Splits, panes, and workspaces.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::command::Direction;
use crate::error::{Error, Result};
use crate::state::{Focus, Pane};
use crate::workspace::layout::{LayoutNode, SplitMode};
use crate::workspace::{MIN_PANE_COLS, MIN_PANE_ROWS, SplitAxis, Workspace};

use super::App;

impl App {
    pub(crate) fn split_pane(&mut self, axis: SplitAxis) -> Result<()> {
        let pane_id = self.focused_or_first_pane()?;
        let area = self.state.ui.panes;
        let new_id = self.state.ids.pane();
        let mut layout = self.state.workspaces.active().layout.clone();
        if !layout.split_pane(pane_id, axis, new_id) {
            return Err(Error::UnknownPane(pane_id));
        }

        let mut rects = HashMap::new();
        layout.layout(area, &mut rects);
        if rects
            .values()
            .any(|rect| rect.width < MIN_PANE_COLS || rect.height < MIN_PANE_ROWS)
        {
            return Err(Error::NoSpaceForSplit);
        }

        let source = self
            .state
            .panes
            .get(&pane_id)
            .ok_or(Error::UnknownPane(pane_id))?;
        let mut pane = Pane::new(new_id);
        pane.tabs = source.tabs.clone();
        pane.active = source.active;
        pane.cursor = source.cursor;
        pane.scroll_top = source.scroll_top;
        pane.scroll_row = 0;
        pane.scroll_left = source.scroll_left;
        pane.tab_scroll = source.tab_scroll;
        pane.tab_follow = source.tab_follow;
        self.state.panes.insert(new_id, pane);

        let workspace = self.state.workspaces.active_mut();
        workspace.layout = layout;
        workspace.focus = new_id;
        self.state.focus = Focus::Pane(new_id);
        self.refresh_layout_rects();
        Ok(())
    }

    /// Recompute pane rectangles from the current pane area. Called after any
    /// layout change so focus, hit-testing, and scrolling never read stale rects.
    pub(crate) fn refresh_layout_rects(&mut self) {
        crate::ui::refresh_panes(&mut self.state);
    }

    pub(crate) fn close_pane(&mut self) -> Result<()> {
        let pane_id = self.focused_or_first_pane()?;
        if self.state.workspaces.active().layout.leaves().len() <= 1 {
            return Err(Error::LastPane);
        }
        let dirty = self
            .state
            .panes
            .get(&pane_id)
            .map(|pane| {
                pane.tabs.iter().any(|id| {
                    self.state
                        .documents
                        .get(id)
                        .map(|document| document.dirty)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if dirty {
            self.notify("pane has unsaved changes; save or close them first");
            return Ok(());
        }
        let mut layout = self.state.workspaces.active().layout.clone();
        if !layout.remove_pane(pane_id) {
            return Err(Error::UnknownPane(pane_id));
        }
        // `leaves()` includes inactive tabs, which are not drawn; focusing one
        // would send typing to a pane the user cannot see.
        let mut visible = std::collections::HashMap::new();
        layout.layout(self.state.ui.panes, &mut visible);
        let next = visible
            .keys()
            .min()
            .copied()
            .or_else(|| layout.leaves().first().copied())
            .ok_or(Error::NoFocusedPane)?;

        let removed = self
            .state
            .panes
            .remove(&pane_id)
            .ok_or(Error::UnknownPane(pane_id))?;
        for id in removed.tabs {
            if !self.state.is_document_referenced(id) {
                self.state.documents.remove(&id);
            }
        }

        let workspace = self.state.workspaces.active_mut();
        workspace.layout = layout;
        workspace.focus = next;
        self.state.focus = Focus::Pane(next);
        if self.state.fullscreen == Some(pane_id) {
            self.state.fullscreen = None;
        }
        self.refresh_layout_rects();
        Ok(())
    }

    pub(crate) fn toggle_fullscreen(&mut self) {
        if self.state.fullscreen.is_some() {
            self.state.fullscreen = None;
            self.notify("fullscreen off");
        } else if let Ok(id) = self.focused_or_first_pane() {
            self.state.fullscreen = Some(id);
            self.notify("fullscreen on: F9 to exit");
        }
        self.refresh_layout_rects();
    }

    pub(crate) fn new_workspace(&mut self) -> Result<()> {
        let pane_id = self.state.ids.pane();
        let workspace_id = self.state.ids.workspace();
        self.state.panes.insert(pane_id, Pane::new(pane_id));
        let root = self.state.workspaces.active().root.clone();
        let name = format!("ws{}", workspace_id.0);
        let index =
            self.state
                .workspaces
                .push(Workspace::new(workspace_id, name, pane_id, root.clone()));
        self.state.workspaces.switch(index);
        self.state.focus = Focus::Pane(pane_id);
        self.state.fullscreen = None;
        self.refresh_layout_rects();
        self.replace_watcher(workspace_id, &root);
        self.notify(format!("workspace {index} created"));
        Ok(())
    }

    pub(crate) fn cycle_workspace(&mut self, delta: isize) -> Result<()> {
        let count = self.state.workspaces.workspaces.len();
        if count <= 1 {
            self.notify("only one workspace");
            return Ok(());
        }
        let index =
            (self.state.workspaces.active as isize + delta).rem_euclid(count as isize) as usize;
        self.switch_workspace(index)
    }

    pub(crate) fn switch_workspace(&mut self, index: usize) -> Result<()> {
        let Some(root) = self
            .state
            .workspaces
            .workspaces
            .get(index)
            .map(|workspace| workspace.root.clone())
        else {
            self.notify(format!("no workspace {}", index + 1));
            return Ok(());
        };
        // Switching commits the layout and panes; a root that no longer exists
        // would leave the explorer and every service on the old project.
        if !root.is_dir() {
            self.notify(format!("workspace root unavailable: {}", root.display()));
            return Ok(());
        }
        if !self.state.workspaces.switch(index) {
            self.notify(format!("no workspace {}", index + 1));
            return Ok(());
        }
        let workspace = self.state.workspaces.active();
        let remembered = workspace.focus;
        let target = if workspace.layout.contains(remembered) {
            remembered
        } else {
            workspace
                .layout
                .leaves()
                .first()
                .copied()
                .ok_or(Error::NoFocusedPane)?
        };
        self.state.workspaces.active_mut().focus = target;
        self.state.focus = Focus::Pane(target);
        self.state.fullscreen = None;
        self.refresh_layout_rects();
        if let Err(error) = self.apply_active_root() {
            self.notify(format!("workspace root unavailable: {error}"));
        }
        self.plugin_event("focus");
        Ok(())
    }

    /// Point the active workspace at another directory and rebuild its explorer.
    pub(crate) fn set_workspace_root(&mut self, root: PathBuf) -> Result<()> {
        let root = root
            .canonicalize()
            .map_err(|error| Error::io("resolve", &root, error))?;
        if !root.is_dir() {
            return Err(Error::InvalidPath(root));
        }
        let id = self.state.workspaces.active().id;
        let old_root = self.state.workspaces.active().root.clone();
        if self.state.workspaces.active().root == root {
            self.notify("workspace already uses that root");
            return Ok(());
        }
        {
            let workspace = self.state.workspaces.active_mut();
            if workspace.terminal_dir == old_root {
                workspace.terminal_dir = root.clone();
            }
            workspace.root = root.clone();
        }
        self.apply_active_root()?;
        self.replace_watcher(id, &root);
        self.state.workspaces.active_mut().name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        self.notify(format!("workspace root: {}", root.display()));
        Ok(())
    }

    pub(crate) fn set_workspace_terminal_dir(&mut self, path: PathBuf) -> Result<()> {
        let path = path
            .canonicalize()
            .map_err(|error| Error::io("resolve", &path, error))?;
        if !path.is_dir() {
            return Err(Error::InvalidPath(path));
        }
        self.state.workspaces.active_mut().terminal_dir = path.clone();
        self.notify(format!("terminal directory: {}", path.display()));
        Ok(())
    }

    pub(crate) fn rename_workspace(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            self.notify("invalid workspace name");
            return Ok(());
        }
        self.state.workspaces.active_mut().name = name.to_string();
        self.notify(format!("workspace renamed to {name}"));
        Ok(())
    }

    pub(crate) fn close_workspace(&mut self) -> Result<()> {
        let active = self.state.workspaces.active;
        let leaves = self.state.workspaces.active().layout.leaves();
        let mut dirty = false;
        for leaf in &leaves {
            if let Some(pane) = self.state.panes.get(leaf) {
                for tab in &pane.tabs {
                    if self
                        .state
                        .documents
                        .get(tab)
                        .map(|document| document.dirty)
                        .unwrap_or(false)
                    {
                        dirty = true;
                    }
                }
            }
        }
        if dirty {
            self.notify("workspace has unsaved changes; save or close them first");
            return Ok(());
        }

        // The workspace that becomes active must still have its root on disk,
        // or the close would leave every service on the old project.
        let count = self.state.workspaces.workspaces.len();
        // Closing the active workspace leaves the next one at the same index.
        let fallback = if active + 1 < count {
            active + 1
        } else {
            active.saturating_sub(1)
        };
        let fallback_root = self
            .state
            .workspaces
            .workspaces
            .get(fallback)
            .map(|workspace| workspace.root.clone());
        if let Some(root) = &fallback_root
            && !root.is_dir()
        {
            self.notify(format!("workspace root unavailable: {}", root.display()));
            return Ok(());
        }

        let removed = self.state.workspaces.close(active)?;
        let closed_id = removed.id;
        self.watchers.retain(|(id, _)| *id != closed_id);
        // Its per-root session file would otherwise restore the closed
        // workspace on the next launch in that directory.
        if !self
            .state
            .workspaces
            .workspaces
            .iter()
            .any(|workspace| workspace.root == removed.root)
            && let Some(path) = crate::session::path_for(&removed.root)
        {
            let _ = std::fs::remove_file(path);
        }
        for leaf in removed.layout.leaves() {
            if let Some(pane) = self.state.panes.remove(&leaf) {
                for id in pane.tabs {
                    if !self.state.is_document_referenced(id) {
                        self.state.documents.remove(&id);
                    }
                }
            }
        }

        let workspace = self.state.workspaces.active();
        let remembered = workspace.focus;
        let target = if workspace.layout.contains(remembered) {
            remembered
        } else {
            workspace
                .layout
                .leaves()
                .first()
                .copied()
                .ok_or(Error::NoFocusedPane)?
        };
        self.state.workspaces.active_mut().focus = target;
        self.state.focus = Focus::Pane(target);
        self.state.fullscreen = None;
        self.refresh_layout_rects();
        if let Err(error) = self.apply_active_root() {
            self.notify(format!("workspace root unavailable: {error}"));
        }
        self.notify(format!("closed workspace {closed_id}"));
        Ok(())
    }

    /// Nearest pane in `direction` that shares an edge with the source.
    fn adjacent_pane(&self, direction: Direction) -> Option<crate::state::PaneId> {
        let current = self.state.focused_pane_id()?;
        let rects = &self.state.ui.pane_rects;
        let from = *rects.get(&current)?;
        let mut best: Option<(crate::state::PaneId, u16)> = None;
        for (id, rect) in rects {
            if *id == current {
                continue;
            }
            let (dx, dy) = (rect.x as i64 - from.x as i64, rect.y as i64 - from.y as i64);
            let vertical_overlap = rect.y < from.y + from.height && from.y < rect.y + rect.height;
            let horizontal_overlap = rect.x < from.x + from.width && from.x < rect.x + rect.width;
            let (overlap, distance, in_plane) = match direction {
                Direction::Left => (vertical_overlap, (-dx).max(0) as u16, dx < 0),
                Direction::Right => (vertical_overlap, dx.max(0) as u16, dx > 0),
                Direction::Up => (horizontal_overlap, (-dy).max(0) as u16, dy < 0),
                Direction::Down => (horizontal_overlap, dy.max(0) as u16, dy > 0),
            };
            if !overlap || !in_plane {
                continue;
            }
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((*id, distance));
            }
        }
        best.map(|(id, _)| id)
    }

    pub(crate) fn focus_direction(&mut self, direction: Direction) -> Result<()> {
        self.focused_or_first_pane()?;
        if let Some(id) = self.adjacent_pane(direction) {
            self.state.focus = Focus::Pane(id);
            self.state.workspaces.active_mut().focus = id;
        }
        Ok(())
    }

    /// Swap the focused pane with its neighbour, keeping the tree and weights.
    pub(crate) fn move_pane(&mut self, direction: Direction) -> Result<()> {
        let current = self.focused_or_first_pane()?;
        let Some(target) = self.adjacent_pane(direction) else {
            self.notify(format!("no pane to the {}", direction_label(direction)));
            return Ok(());
        };
        if !self
            .state
            .workspaces
            .active_mut()
            .layout
            .swap_panes(current, target)
        {
            return Ok(());
        }
        self.state.focus = Focus::Pane(current);
        self.state.workspaces.active_mut().focus = current;
        self.refresh_layout_rects();
        self.notify(format!(
            "moved pane {current} {}",
            direction_label(direction)
        ));
        Ok(())
    }

    pub(crate) fn toggle_move_mode(&mut self) {
        self.state.moving = !self.state.moving;
        if self.state.moving {
            self.notify("MOVE: arrows move the pane, Esc or F10 exits");
        } else {
            self.notify("move mode off");
        }
    }

    /// Activate one tab of the tabbed container at `path`.
    pub(crate) fn activate_tab(&mut self, path: &[usize], index: usize) {
        let focus_leaf = {
            let Some(node) = self.state.workspaces.active_mut().layout.node_at(path) else {
                return;
            };
            let LayoutNode::Split {
                mode,
                active,
                children,
                ..
            } = node
            else {
                return;
            };
            if *mode != SplitMode::Tabbed || index >= children.len() {
                return;
            }
            *active = index;
            children[index].leaves().first().copied()
        };
        if let Some(leaf) = focus_leaf {
            self.state.focus = Focus::Pane(leaf);
            self.state.workspaces.active_mut().focus = leaf;
        }
        self.refresh_layout_rects();
    }

    /// Convert the innermost split around the focused pane to tabbed and back.
    pub(crate) fn toggle_tabbed(&mut self) {
        let Ok(pane) = self.focused_or_first_pane() else {
            return;
        };
        if !self
            .state
            .workspaces
            .active_mut()
            .layout
            .toggle_tabbed(pane)
        {
            self.notify("split a pane first, then press F12");
            return;
        }
        self.refresh_layout_rects();
        if !self.state.ui.pane_rects.contains_key(&pane) {
            let visible = self
                .state
                .workspaces
                .active()
                .layout
                .leaves()
                .into_iter()
                .find(|id| self.state.ui.pane_rects.contains_key(id));
            if let Some(visible) = visible {
                self.state.focus = Focus::Pane(visible);
                self.state.workspaces.active_mut().focus = visible;
            }
        }
        self.notify("layout toggled (tiled <-> tabbed)");
    }

    /// Switch tabs inside the innermost tabbed container around the focus.
    pub(crate) fn cycle_pane_tab(&mut self, delta: isize) {
        let Ok(pane) = self.focused_or_first_pane() else {
            return;
        };
        let next = self
            .state
            .workspaces
            .active_mut()
            .layout
            .cycle_tab(pane, delta);
        match next {
            Some(leaf) => {
                self.state.focus = Focus::Pane(leaf);
                self.state.workspaces.active_mut().focus = leaf;
                self.refresh_layout_rects();
            }
            None => self.notify("no tabbed container here; press F12 to create one"),
        }
    }
}

fn direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}
