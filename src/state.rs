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

#[allow(clippy::large_enum_variant)] // PromptState is moved, not cloned, on submit.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub cursor: usize,
    pub anchor: Option<usize>,
}

impl Cursor {
    pub fn range(self) -> std::ops::Range<usize> {
        match self.anchor {
            Some(anchor) if anchor < self.cursor => anchor..self.cursor,
            Some(anchor) if anchor > self.cursor => self.cursor..anchor,
            _ => self.cursor..self.cursor,
        }
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
    /// Horizontal offset of the document tab strip, in cells.
    pub tab_scroll: u16,
    /// Keep the active tab in view until the user scrolls the strip manually.
    pub tab_follow: bool,
    pub target_col: Option<usize>,
    /// Secondary cursors/selections, kept in no particular order.
    pub extra: Vec<Cursor>,
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
            tab_scroll: 0,
            tab_follow: true,
            target_col: None,
            extra: Vec::new(),
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

    /// Every cursor as a range: primary first, then secondary carets and
    /// selections. Overlapping ranges are merged so an edit never applies twice
    /// to one character.
    pub fn edit_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::with_capacity(self.extra.len() + 1);
        ranges.push(match self.selection() {
            Some(range) => range,
            None => self.cursor..self.cursor,
        });
        ranges.extend(self.extra.iter().map(|cursor| cursor.range()));
        ranges.sort_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    }

    /// Ranges that carry a real selection, for copy/cut.
    pub fn selected_ranges(&self) -> Vec<Range<usize>> {
        self.edit_ranges()
            .into_iter()
            .filter(|range| !range.is_empty())
            .collect()
    }

    pub fn clear_extra(&mut self) {
        self.extra.clear();
    }

    pub fn add_cursor(&mut self, cursor: usize) {
        if cursor == self.cursor || self.extra.iter().any(|extra| extra.cursor == cursor) {
            return;
        }
        self.extra.push(Cursor {
            cursor,
            anchor: None,
        });
    }

    /// Rectangular selection: one range per line between `start` and `end`,
    /// using the same character columns on every line.
    pub fn column_selection(
        &mut self,
        document: &Document,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) {
        let last = document.line_count().saturating_sub(1);
        let (first, last_line) = if start_line <= end_line {
            (start_line.min(last), end_line.min(last))
        } else {
            (end_line.min(last), start_line.min(last))
        };
        let (left, right) = if start_col <= end_col {
            (start_col, end_col)
        } else {
            (end_col, start_col)
        };
        self.extra.clear();
        for line in first..=last_line {
            let text = document.line_text(line);
            let length = text.chars().count();
            let anchor = document.char_of_line(line) + left.min(length);
            let cursor = document.char_of_line(line) + right.min(length);
            if line == first {
                self.cursor = cursor;
                self.anchor = Some(anchor);
            } else {
                self.extra.push(Cursor {
                    cursor,
                    anchor: Some(anchor),
                });
            }
        }
        self.target_col = None;
    }

    /// Place the primary cursor after a multi-selection edit and rebuild the
    /// secondary carets. `ranges` are the merged, ascending edit ranges.
    pub fn apply_edit_result(&mut self, ranges: &[Range<usize>], inserted: usize) {
        let positions = cursors_after(ranges, inserted);
        let Some((&last, rest)) = positions.split_last() else {
            return;
        };
        self.cursor = last;
        self.anchor = None;
        self.extra = rest
            .iter()
            .map(|cursor| Cursor {
                cursor: *cursor,
                anchor: None,
            })
            .collect();
        self.target_col = None;
    }

    pub fn select_all(&mut self, document: &Document) {
        self.anchor = Some(0);
        self.cursor = document.rope().len_chars();
        self.extra.clear();
    }
}

/// Final cursor positions for edits applied to `ranges` (ascending, merged)
/// that insert `length` chars at each start. Selections are replaced, so each
/// position lands after its own inserted text; earlier edits shift later ones.
pub fn cursors_after(ranges: &[Range<usize>], length: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(ranges.len());
    let mut removed_before = 0usize;
    for (index, range) in ranges.iter().enumerate() {
        out.push(range.start + (index + 1) * length - removed_before);
        removed_before += range.end - range.start;
    }
    out
}

/// Result set of the background project search.
#[derive(Debug, Default)]
pub struct ProjectSearch {
    pub query: String,
    pub options: crate::search_options::Options,
    pub results: Vec<crate::project_search::Match>,
    pub truncated: bool,
}

/// Read-only diff overlay.
#[derive(Clone, Debug)]
pub struct DiffView {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: usize,
}

/// Scrollable keybinding overlay.
#[derive(Clone, Debug)]
pub struct HelpView {
    pub lines: Vec<String>,
    pub scroll: usize,
}
#[derive(Clone, Debug)]
pub struct AiToolCall {
    pub id: String,
    pub server: String,
    pub tool: String,
    pub capability: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct AiWaiting {
    pub id: String,
    pub server: String,
    pub tool: String,
    pub capability: String,
}

#[derive(Clone, Debug)]
pub struct AiAttachment {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct AiPanel {
    pub visible: bool,
    pub focused: bool,
    pub entries: Vec<String>,
    pub input: String,
    pub cursor: usize,
    pub scroll: usize,
    pub scroll_x: usize,
    pub busy: bool,
    pub conversation: Vec<serde_json::Value>,
    pub pending: std::collections::VecDeque<AiToolCall>,
    pub waiting: Option<AiWaiting>,
    pub rounds: usize,
    pub queued_question: Option<String>,
    pub attachments: Vec<AiAttachment>,
    pub tool_map: std::collections::BTreeMap<String, (String, String)>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Selectable hunk list for partial staging.
#[derive(Clone, Debug)]
pub struct HunkPicker {
    pub title: String,
    pub relative: String,
    /// True when the selected hunk will be staged; false unstages it.
    pub stage: bool,
    pub hunks: Vec<crate::git::Hunk>,
    pub selected: usize,
    pub scroll: usize,
}

/// Completion rows rendered above the prompt layer.
#[derive(Clone, Debug)]
pub struct CompletionPopup {
    /// Document and text revision the items were requested for. Accepting a
    /// popup whose revision moved on would splice text into a changed buffer.
    pub document: DocumentId,
    pub revision: u64,
    pub items: Vec<crate::lsp::CompletionItem>,
    pub selected: usize,
}

pub struct Search {
    /// Document the offsets belong to; the same buffer may be open in two panes.
    pub document: DocumentId,
    pub query: String,
    pub options: crate::search_options::Options,
    /// Char ranges of every non-overlapping match.
    pub matches: Vec<std::ops::Range<usize>>,
    /// Index into `matches` of the match under the cursor.
    pub index: usize,
    /// True when matches existed past the cap; the UI marks the count.
    pub truncated: bool,
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
    pub search_options: crate::search_options::Options,
    pub project_search: Option<ProjectSearch>,
    /// Diagnostics per document from the last publishDiagnostics notification.
    pub lsp_diagnostics: HashMap<DocumentId, Vec<crate::lsp::Diagnostic>>,
    /// Completion popup, when a server answered the request.
    pub completions: Option<CompletionPopup>,
    /// Hover text popup.
    pub hover: Option<String>,
    /// Per-line git markers for documents shown in a pane.
    pub git_marks: HashMap<DocumentId, Vec<Option<crate::git::Change>>>,
    /// Read-only diff overlay.
    pub git_diff: Option<DiffView>,
    /// Keybinding overlay.
    pub help: Option<HelpView>,
    /// Read-only MCP status overlay.
    pub mcp_view: Option<HelpView>,
    /// Read-only workspace-rules overlay for the AI prompt.
    pub rules_view: Option<HelpView>,
    /// Read-only model-request trace overlay.
    pub trace_view: Option<HelpView>,
    /// AI assistant panel.
    pub ai: AiPanel,
    /// Partial-staging overlay.
    pub hunks: Option<HunkPicker>,
    /// Statusline strings produced by Lua plugins.
    pub plugin_status: Vec<String>,
    /// Pane temporarily occupying the whole pane area.
    pub fullscreen: Option<PaneId>,
    /// Grab mode: arrow keys move the focused pane.
    pub moving: bool,
    /// Bottom terminal panel; empty when closed. Dropping a shell kills it.
    pub terminals: crate::shell::Terminals,
    /// Directory scanned for `<name>.toml` themes.
    pub themes_dir: std::path::PathBuf,
    /// User config file, when one can be resolved; theme choices are written here.
    pub config_path: Option<std::path::PathBuf>,
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
