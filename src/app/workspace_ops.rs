//! Splits, panes, and workspaces.

use std::collections::HashMap;

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
        pane.scroll_left = source.scroll_left;
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
        let mut layout = self.state.workspaces.active().layout.clone();
        if !layout.remove_pane(pane_id) {
            return Err(Error::UnknownPane(pane_id));
        }
        let next = layout
            .leaves()
            .first()
            .copied()
            .ok_or(Error::NoFocusedPane)?;

        let removed = self
            .state
            .panes
            .remove(&pane_id)
            .ok_or(Error::UnknownPane(pane_id))?;
        for id in removed.tabs {
            if !self.state.is_document_referenced(id) {
                let dirty = self
                    .state
                    .documents
                    .get(&id)
                    .map(|document| document.dirty)
                    .unwrap_or(false);
                if !dirty {
                    self.state.documents.remove(&id);
                }
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
        let name = format!("ws{}", workspace_id.0);
        let index = self
            .state
            .workspaces
            .push(Workspace::new(workspace_id, name, pane_id));
        self.state.workspaces.switch(index);
        self.state.focus = Focus::Pane(pane_id);
        self.state.fullscreen = None;
        self.refresh_layout_rects();
        self.notify(format!("workspace {index} created"));
        Ok(())
    }

    pub(crate) fn switch_workspace(&mut self, index: usize) -> Result<()> {
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

        let removed = self.state.workspaces.close(active)?;
        let closed_id = removed.id;
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
