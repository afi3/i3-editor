use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use crate::config::AppConfig;
use crate::editor::document::Document;
use crate::explorer::ExplorerState;
use crate::input::{KeyMap, PromptState};
use crate::theme::Theme;
use crate::ui::UiState;
use crate::workspace::WorkspaceManager;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(DocumentId);
id_type!(PaneId);
id_type!(WorkspaceId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Explorer,
    Pane(PaneId),
    Shell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Prompt(PromptState),
}

impl InputMode {
    pub fn is_prompt(&self) -> bool {
        matches!(self, InputMode::Prompt(_))
    }
}

#[derive(Debug, Default)]
pub struct Ids {
    next_document: u64,
    next_pane: u64,
    next_workspace: u64,
}

impl Ids {
    pub fn document(&mut self) -> DocumentId {
        self.next_document += 1;
        DocumentId(self.next_document)
    }

    pub fn pane(&mut self) -> PaneId {
        self.next_pane += 1;
        PaneId(self.next_pane)
    }

    pub fn workspace(&mut self) -> WorkspaceId {
        self.next_workspace += 1;
        WorkspaceId(self.next_workspace)
    }
}

/// One editor viewport. Cursor, selection, and scroll belong to the pane, never
/// to the document, so two panes can show one buffer at different places.
#[derive(Debug)]
pub struct Pane {
    pub id: PaneId,
    pub tabs: Vec<DocumentId>,
    pub active: Option<DocumentId>,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub scroll_top: usize,
    pub scroll_left: u16,
    pub target_col: Option<usize>,
}

impl Pane {
    pub fn new(id: PaneId) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            active: None,
            cursor: 0,
            anchor: None,
            scroll_top: 0,
            scroll_left: 0,
            target_col: None,
        }
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(if anchor < self.cursor {
            anchor..self.cursor
        } else {
            self.cursor..anchor
        })
    }

    pub fn select_all(&mut self, document: &Document) {
        self.anchor = Some(0);
        self.cursor = document.rope.len_chars();
    }
}

pub struct Search {
    pub query: String,
    pub matches: Vec<usize>,
    pub index: usize,
}

pub struct AppState {
    pub should_quit: bool,
    pub explorer: ExplorerState,
    pub documents: HashMap<DocumentId, Document>,
    pub panes: HashMap<PaneId, Pane>,
    pub workspaces: WorkspaceManager,
    pub focus: Focus,
    pub mode: InputMode,
    pub keymap: KeyMap,
    pub ui: UiState,
    pub config: AppConfig,
    pub theme: Theme,
    pub ids: Ids,
    pub warnings: Vec<String>,
    pub clipboard: String,
    pub search: Option<Search>,
    /// Pane temporarily occupying the whole pane area.
    pub fullscreen: Option<PaneId>,
    /// Grab mode: arrow keys move the focused pane.
    pub moving: bool,
    /// Bottom terminal panel. Dropping it kills the shell.
    pub shell: Option<crate::shell::ShellPanel>,
    /// Directory scanned for `<name>.toml` themes.
    pub themes_dir: std::path::PathBuf,
}

impl AppState {
    pub fn focused_pane(&self) -> Option<&Pane> {
        match self.focus {
            Focus::Pane(id) => self.panes.get(&id),
            Focus::Explorer | Focus::Shell => None,
        }
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        match self.focus {
            Focus::Pane(id) => self.panes.get_mut(&id),
            Focus::Explorer | Focus::Shell => None,
        }
    }

    pub fn focused_pane_id(&self) -> Option<PaneId> {
        match self.focus {
            Focus::Pane(id) => Some(id),
            Focus::Explorer | Focus::Shell => None,
        }
    }

    pub fn active_document_id(&self) -> Option<DocumentId> {
        self.focused_pane().and_then(|pane| pane.active)
    }

    pub fn active_document(&self) -> Option<&Document> {
        let id = self.active_document_id()?;
        self.documents.get(&id)
    }

    pub fn pane_and_document(&mut self) -> Option<(&mut Pane, &mut Document)> {
        let AppState {
            documents, panes, ..
        } = self;
        let pane_id = match self.focus {
            Focus::Pane(id) => id,
            Focus::Explorer | Focus::Shell => return None,
        };
        let pane = panes.get_mut(&pane_id)?;
        let doc_id = pane.active?;
        let document = documents.get_mut(&doc_id)?;
        Some((pane, document))
    }

    /// Live documents are reachable from at least one pane tab.
    pub fn is_document_referenced(&self, id: DocumentId) -> bool {
        self.panes.values().any(|pane| pane.tabs.contains(&id))
    }
}
