//! Workspaces: a named layout tree plus the pane that owns focus.

pub mod layout;

pub use layout::{LayoutNode, MIN_PANE_COLS, MIN_PANE_ROWS, SplitAxis};

use crate::error::{Error, Result};
use crate::state::{PaneId, WorkspaceId};

#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: LayoutNode,
    /// Pane focus remembered when another workspace or the explorer takes over.
    pub focus: PaneId,
    /// Explorer root and watcher scope for this workspace.
    pub root: std::path::PathBuf,
}

impl Workspace {
    pub fn new(
        id: WorkspaceId,
        name: impl Into<String>,
        pane: PaneId,
        root: std::path::PathBuf,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            layout: LayoutNode::Pane(pane),
            focus: pane,
            root,
        }
    }
}

#[derive(Debug, Default)]
pub struct WorkspaceManager {
    pub workspaces: Vec<Workspace>,
    pub active: usize,
}

impl WorkspaceManager {
    pub fn active(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active]
    }

    pub fn switch(&mut self, index: usize) -> bool {
        if index < self.workspaces.len() {
            self.active = index;
            true
        } else {
            false
        }
    }

    pub fn push(&mut self, workspace: Workspace) -> usize {
        self.workspaces.push(workspace);
        self.workspaces.len() - 1
    }

    pub fn close(&mut self, index: usize) -> Result<Workspace> {
        if index >= self.workspaces.len() {
            return Err(Error::UnknownWorkspace(index));
        }
        if self.workspaces.len() == 1 {
            return Err(Error::LastWorkspace);
        }
        let workspace = self.workspaces.remove(index);
        if self.active >= self.workspaces.len() {
            self.active = self.workspaces.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }
        Ok(workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> WorkspaceManager {
        let mut manager = WorkspaceManager::default();
        manager.push(Workspace::new(
            WorkspaceId(1),
            "one",
            PaneId(1),
            std::path::PathBuf::from("/tmp"),
        ));
        manager.push(Workspace::new(
            WorkspaceId(2),
            "two",
            PaneId(2),
            std::path::PathBuf::from("/tmp"),
        ));
        manager
    }

    #[test]
    fn switch_only_accepts_existing_indices() {
        let mut manager = manager();
        assert!(manager.switch(1));
        assert_eq!(manager.active().id, WorkspaceId(2));
        assert!(!manager.switch(9));
        assert_eq!(manager.active().id, WorkspaceId(2));
    }

    #[test]
    fn closing_the_active_workspace_falls_back() {
        let mut manager = manager();
        manager.switch(1);
        manager.close(1).expect("close");
        assert_eq!(manager.workspaces.len(), 1);
        assert_eq!(manager.active, 0);
    }

    #[test]
    fn the_last_workspace_cannot_be_closed() {
        let mut manager = manager();
        manager.close(0).expect("close");
        assert!(matches!(manager.close(0), Err(Error::LastWorkspace)));
    }
}
