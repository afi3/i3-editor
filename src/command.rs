//! Commands are the only way to mutate application state.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Quit,
    Save,
    SavePrompt,
    OpenPrompt,
    NewPrompt,
    NewScratch,
    CloseDocument,
    ToggleExplorer,
    QuickOpen,
    Palette,
    Messages,
    Help,
    Mcp,
    GoToLinePrompt,
    FindPrompt,
    FindInFiles,
    ReplacePrompt,
    ReplaceInFiles,
    NextDocument,
    PrevDocument,
    NextMatch,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Paste,
    DuplicateLine,
    MoveLinesUp,
    MoveLinesDown,
    DeleteLine,
    ToggleComment,
    ResolveConflictOurs,
    ResolveConflictTheirs,
    ResolveConflictBoth,
    ToggleFold,
    GoToSymbol,
    AddCursorBelow,
    AddCursorAbove,
    ClearCursors,
    ToggleWordWrap,
    LineEndingsLf,
    LineEndingsCrlf,
    LspCompletion,
    LspHover,
    LspDefinition,
    LspInstall,
    LspUninstall,
    McpAdd,
    McpResources,
    McpResourceTemplates,
    McpPrompts,
    AiChat,
    AiPanel,
    AiClear,
    AiCopy,
    AiWider,
    AiNarrower,
    AiScrollLeft,
    AiScrollRight,
    AiAttachFile,
    AiAttachSelection,
    AiClearContext,
    AiUndoEdit,
    AiBackground,
    AiObjectiveAdd,
    AiObjectives,
    AiQueueObjectives,
    AiObjectiveLink,
    AiIndexProject,
    AiContextFiles,
    AiRules,
    AiTrace,
    GitStatus,
    GitDiff,
    GitLog,
    GitPull,
    GitPush,
    GitConflicts,
    GitStage,
    GitUnstage,
    GitStageHunk,
    GitUnstageHunk,
    GitCommit,
    InsertText(String),
    InsertTab,
    InsertNewline,
    Backspace,
    DeleteForward,
    MoveCursor(Move),
    ExtendSelection(Move),
    FocusPane(Direction),
    MovePane(Direction),
    ToggleMoveMode,
    ExplorerUp,
    ExplorerDown,
    ExplorerSelect,
    ExplorerExpand,
    ExplorerCollapse,
    ExplorerNewFile,
    ExplorerNewDir,
    ExplorerRename,
    ExplorerDelete,
    ExplorerRefresh,
    ExplorerToggleHidden,
    DuplicateFile,
    CopyPath,
    RevealInTerminal,
    SplitLeftRight,
    SplitTopBottom,
    ClosePane,
    ToggleFullscreen,
    ToggleTabbed,
    NextPaneTab,
    PrevPaneTab,
    ToggleTerminal,
    TerminalNew,
    TerminalClose,
    NextTerminal,
    PrevTerminal,
    TerminalRename,
    SelectTheme,
    SelectAiProvider,
    McpUnsubscribe,
    AiSemanticIndex,
    AiSemanticStatus,
    NewWorkspace,
    SetWorkspaceRoot,
    SetWorkspaceTerminalDir,
    RenameWorkspace,
    CloseWorkspace,
    SwitchWorkspace(usize),
    NextWorkspace,
    PrevWorkspace,
    SelectWorkspace,
    SaveTo(PathBuf),
    CreateFile(PathBuf),
    CreateDir(PathBuf),
    RenameTo(PathBuf),
    DeleteTarget(PathBuf),
}

impl Command {
    /// Commands that would mutate the active buffer. Read-only documents
    /// refuse these before dispatch.
    pub fn edits_document(&self) -> bool {
        matches!(
            self,
            Command::Undo
                | Command::Redo
                | Command::Cut
                | Command::Paste
                | Command::DuplicateLine
                | Command::MoveLinesUp
                | Command::MoveLinesDown
                | Command::DeleteLine
                | Command::ToggleComment
                | Command::ResolveConflictOurs
                | Command::ResolveConflictTheirs
                | Command::ResolveConflictBoth
                | Command::ToggleFold
                | Command::LineEndingsLf
                | Command::LineEndingsCrlf
                | Command::InsertText(_)
                | Command::InsertTab
                | Command::InsertNewline
                | Command::Backspace
                | Command::DeleteForward
                | Command::AiUndoEdit
        )
    }

    pub fn name(&self) -> &'static str {
        self.spec().0
    }

    pub fn description(&self) -> &'static str {
        self.spec().1
    }

    /// One place for every command's name and palette description. The groups
    /// keep each match small enough to review; `unmatched_spec` is a fallback
    /// that the `every_command_has_a_specification` test makes unreachable.
    fn spec(&self) -> (&'static str, &'static str) {
        self.primary_spec()
            .or_else(|| self.secondary_spec())
            .unwrap_or(("unknown", "Unknown command"))
    }

    fn primary_spec(&self) -> Option<(&'static str, &'static str)> {
        self.search_spec()
            .or_else(|| self.git_spec())
            .or_else(|| self.lsp_spec())
            .or_else(|| self.edit_spec())
            .or_else(|| self.document_spec())
            .or_else(|| self.navigation_spec())
            .or_else(|| self.file_spec())
            .or_else(|| self.explorer_spec())
    }

    fn file_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::DuplicateFile => ("duplicate_file", "Duplicate the selected file"),
            Command::CopyPath => ("copy_path", "Copy the selected path"),
            Command::RevealInTerminal => (
                "reveal_in_terminal",
                "Reveal the selected path in the terminal",
            ),
            _ => return None,
        })
    }

    fn edit_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::ToggleComment => ("comment", "Comment or uncomment the selection"),
            Command::ResolveConflictOurs => (
                "resolve_conflict_ours",
                "Resolve the conflict at/after the cursor with ours",
            ),
            Command::ResolveConflictTheirs => (
                "resolve_conflict_theirs",
                "Resolve the conflict at/after the cursor with theirs",
            ),
            Command::ResolveConflictBoth => (
                "resolve_conflict_both",
                "Resolve the conflict at/after the cursor with both sides",
            ),
            Command::ToggleFold => ("fold", "Fold or unfold the block at the cursor"),
            Command::AddCursorBelow => ("add_cursor_below", "Add a cursor on the next line"),
            Command::AddCursorAbove => ("add_cursor_above", "Add a cursor on the previous line"),
            Command::ClearCursors => ("clear_cursors", "Remove secondary cursors"),
            Command::ToggleWordWrap => ("word_wrap", "Turn soft word wrap on or off"),
            Command::LineEndingsLf => ("line_endings_lf", "Convert the document to LF"),
            Command::LineEndingsCrlf => ("line_endings_crlf", "Convert the document to CRLF"),
            Command::GoToSymbol => ("go_to_symbol", "Jump to a definition in the document"),
            _ => return None,
        })
    }

    fn git_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::GitStatus => ("git_status", "List changed files; Enter opens the diff"),
            Command::GitDiff => ("git_diff", "Show the diff for the active file"),
            Command::GitLog => ("git_log", "Show branches and recent commits"),
            Command::GitPull => ("git_pull", "Fast-forward the current branch from upstream"),
            Command::GitPush => ("git_push", "Push the current branch to upstream"),
            Command::GitConflicts => ("git_conflicts", "List and open merge conflicts"),
            Command::GitStage => ("git_stage", "Stage the active file"),
            Command::GitUnstage => ("git_unstage", "Unstage the active file"),
            Command::GitStageHunk => ("git_stage_hunk", "Stage one hunk of the active file"),
            Command::GitUnstageHunk => ("git_unstage_hunk", "Unstage one hunk of the active file"),
            Command::GitCommit => ("git_commit", "Commit the staged changes"),
            _ => return None,
        })
    }

    fn ai_spec(&self) -> Option<(&'static str, &'static str)> {
        self.ai_spec_panel().or_else(|| self.ai_spec_workflow())
    }

    fn ai_spec_panel(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::AiChat => ("ai_chat", "Open and focus the AI panel"),
            Command::AiPanel => ("ai_panel", "Show or hide the AI panel"),
            Command::AiClear => ("ai_clear", "Clear the AI conversation"),
            Command::AiCopy => ("ai_copy", "Copy the AI transcript"),
            Command::AiWider => ("ai_wider", "Widen the AI panel"),
            Command::AiNarrower => ("ai_narrower", "Narrow the AI panel"),
            Command::AiScrollLeft => ("ai_scroll_left", "Scroll the AI transcript left"),
            Command::AiScrollRight => ("ai_scroll_right", "Scroll the AI transcript right"),
            _ => return None,
        })
    }

    fn ai_spec_workflow(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::AiAttachFile => (
                "ai_attach_file",
                "Attach the active file to the next AI prompt",
            ),
            Command::AiAttachSelection => (
                "ai_attach_selection",
                "Attach the editor selection to the next AI prompt",
            ),
            Command::AiClearContext => ("ai_clear_context", "Clear pending AI attachments"),
            Command::AiUndoEdit => ("ai_undo_edit", "Undo the last native AI edit"),
            Command::AiBackground => ("ai_background", "Start a read-only background agent"),
            Command::AiObjectiveAdd => ("ai_objective_add", "Add a persistent agent objective"),
            Command::AiObjectives => ("ai_objectives", "List and run agent objectives"),
            Command::AiQueueObjectives => {
                ("ai_queue_objectives", "Run pending objectives in sequence")
            }
            Command::AiObjectiveLink => ("ai_objective_link", "Link objective dependencies"),
            Command::AiIndexProject => ("ai_index_project", "Build the workspace metadata index"),
            Command::AiContextFiles => {
                ("ai_context_files", "Choose an indexed file for AI context")
            }
            Command::AiRules => ("ai_rules", "Show the workspace AI rules file"),
            Command::AiTrace => ("ai_trace", "Show recent model request traces"),
            _ => return None,
        })
    }

    fn lsp_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::LspCompletion => ("lsp_completion", "Ask the language server for completions"),
            Command::LspHover => ("lsp_hover", "Show type information under the cursor"),
            Command::LspDefinition => (
                "lsp_definition",
                "Jump to the definition via the language server",
            ),
            Command::LspInstall => (
                "lsp_install",
                "Install the language server for the active file",
            ),
            Command::McpAdd => ("mcp_add", "Add a configured MCP server"),
            Command::McpResources => ("mcp_resources", "List MCP resources"),
            Command::McpResourceTemplates => {
                ("mcp_resource_templates", "List MCP resource templates")
            }
            Command::McpPrompts => ("mcp_prompts", "List MCP prompts"),
            Command::AiChat
            | Command::AiPanel
            | Command::AiClear
            | Command::AiCopy
            | Command::AiWider
            | Command::AiNarrower
            | Command::AiScrollLeft
            | Command::AiScrollRight
            | Command::AiAttachFile
            | Command::AiAttachSelection
            | Command::AiClearContext
            | Command::AiUndoEdit
            | Command::AiBackground
            | Command::AiObjectiveAdd
            | Command::AiObjectives
            | Command::AiQueueObjectives
            | Command::AiObjectiveLink
            | Command::AiIndexProject
            | Command::AiContextFiles
            | Command::AiRules
            | Command::AiTrace => return self.ai_spec(),
            Command::LspUninstall => (
                "lsp_uninstall",
                "Remove the installed language server for the active file",
            ),
            _ => return None,
        })
    }

    fn search_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::GoToLinePrompt => ("go_to_line", "Jump to a line"),
            Command::FindPrompt => ("find", "Find in the active document"),
            Command::FindInFiles => ("find_in_files", "Search the project for text"),
            Command::ReplacePrompt => ("replace", "Replace in the active document"),
            Command::ReplaceInFiles => (
                "replace_in_files",
                "Replace in files (Ctrl+Enter applies to the selected file)",
            ),
            _ => return None,
        })
    }

    fn secondary_spec(&self) -> Option<(&'static str, &'static str)> {
        self.text_spec()
            .or_else(|| self.layout_spec())
            .or_else(|| self.terminal_spec())
            .or_else(|| self.workspace_spec())
    }

    fn document_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::Quit => ("quit", "Quit i3-editor"),
            Command::Save => ("save", "Save the active document"),
            Command::SavePrompt => ("save_as", "Save the active document as..."),
            Command::OpenPrompt => ("open", "Open a path"),
            Command::NewPrompt => ("new_file", "Create a new file"),
            Command::NewScratch => ("new_scratch", "New scratch buffer"),
            Command::CloseDocument => ("close_document", "Close the active document tab"),
            Command::ToggleExplorer => ("explorer", "Show or hide the explorer"),
            Command::Palette => ("command_palette", "Run a command"),
            Command::Messages => ("messages", "Show startup warnings"),
            Command::Help => ("help", "Show keybindings"),
            Command::Mcp => ("mcp", "Show MCP servers and status"),
            _ => return None,
        })
    }

    fn navigation_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::QuickOpen => ("quick_open", "Fuzzy open a file"),
            Command::NextDocument => ("next_document", "Next document tab"),
            Command::PrevDocument => ("prev_document", "Previous document tab"),
            Command::NextMatch => ("next_match", "Jump to the next match"),
            Command::Undo => ("undo", "Undo"),
            Command::Redo => ("redo", "Redo"),
            Command::SelectAll => ("select_all", "Select all"),
            Command::Copy => ("copy", "Copy selection"),
            Command::Cut => ("cut", "Cut selection"),
            Command::Paste => ("paste", "Paste"),
            Command::DuplicateLine => ("duplicate_line", "Duplicate the current line"),
            _ => return None,
        })
    }

    fn text_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::InsertText(_) => ("insert_text", "Insert text"),
            Command::InsertTab => ("insert_tab", "Insert indentation at the cursor"),
            Command::InsertNewline => ("insert_newline", "New line"),
            Command::Backspace => ("backspace", "Delete backwards"),
            Command::DeleteForward => ("delete_forward", "Delete forwards"),
            Command::MoveCursor(_) => ("move_cursor", "Move the cursor"),
            Command::ExtendSelection(_) => ("extend_selection", "Extend the selection"),
            Command::MoveLinesUp => ("move_lines_up", "Move the current line or selection up"),
            Command::MoveLinesDown => {
                ("move_lines_down", "Move the current line or selection down")
            }
            Command::DeleteLine => ("delete_line", "Delete the current line"),
            _ => return None,
        })
    }

    fn explorer_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::ExplorerUp => ("explorer_up", "Select the previous entry"),
            Command::ExplorerDown => ("explorer_down", "Select the next entry"),
            Command::ExplorerSelect => ("explorer_select", "Open the selected entry"),
            Command::ExplorerExpand => ("explorer_expand", "Expand the selected directory"),
            Command::ExplorerCollapse => ("explorer_collapse", "Collapse the selected directory"),
            Command::ExplorerNewFile => ("explorer_new_file", "New file in the selected directory"),
            Command::ExplorerNewDir => (
                "explorer_new_dir",
                "New directory in the selected directory",
            ),
            Command::ExplorerRename => ("explorer_rename", "Rename the selected entry"),
            Command::ExplorerDelete => ("explorer_delete", "Delete the selected entry"),
            Command::ExplorerRefresh => ("explorer_refresh", "Reload the explorer"),
            Command::ExplorerToggleHidden => {
                ("hidden_files", "Show or hide hidden and ignored files")
            }
            _ => return None,
        })
    }

    fn layout_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::FocusPane(_) => ("focus_pane", "Move focus to another pane"),
            Command::MovePane(direction) => (
                direction_name(*direction),
                "Move the focused pane in a direction",
            ),
            Command::ToggleMoveMode => ("move_mode", "Grab the focused pane (arrows move it)"),
            Command::SplitLeftRight => ("split_left_right", "Split pane left/right"),
            Command::SplitTopBottom => ("split_top_bottom", "Split pane top/bottom"),
            Command::ClosePane => ("close_pane", "Close the focused pane"),
            Command::ToggleFullscreen => ("fullscreen", "Make the focused pane fullscreen"),
            Command::ToggleTabbed => ("tabbed", "Use a tabbed layout for this container"),
            Command::NextPaneTab => ("next_pane_tab", "Next tab in the tabbed container"),
            Command::PrevPaneTab => ("prev_pane_tab", "Previous tab in the tabbed container"),
            _ => return None,
        })
    }

    fn terminal_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::ToggleTerminal => ("terminal", "Open or close the bottom terminal"),
            Command::TerminalNew => ("new_terminal", "Open another terminal tab"),
            Command::TerminalClose => ("close_terminal", "Close the active terminal tab"),
            Command::NextTerminal => ("next_terminal", "Next terminal tab"),
            Command::PrevTerminal => ("prev_terminal", "Previous terminal tab"),
            Command::TerminalRename => ("rename_terminal", "Rename the active terminal tab"),
            Command::SelectTheme => ("select_theme", "Pick a theme from the themes directory"),
            Command::SelectAiProvider => ("select_ai_provider", "Pick an AI provider"),
            Command::McpUnsubscribe => {
                ("mcp_unsubscribe", "Release every MCP resource subscription")
            }
            Command::AiSemanticIndex => (
                "ai_semantic_index",
                "Rebuild the semantic memory index from open files",
            ),
            Command::AiSemanticStatus => {
                ("ai_semantic_status", "Show the semantic memory index state")
            }
            _ => return None,
        })
    }

    fn workspace_spec(&self) -> Option<(&'static str, &'static str)> {
        self.workspace_edit_spec()
            .or_else(|| self.workspace_navigation_spec())
    }

    fn workspace_edit_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::NewWorkspace => ("new_workspace", "Create a workspace"),
            Command::SetWorkspaceRoot => (
                "workspace_root",
                "Change the explorer root of the active workspace",
            ),
            Command::SetWorkspaceTerminalDir => (
                "workspace_terminal_dir",
                "Set the terminal directory for the active workspace",
            ),
            Command::CloseWorkspace => ("close_workspace", "Close the active workspace"),
            Command::RenameWorkspace => ("rename_workspace", "Rename the active workspace"),
            Command::SaveTo(_) => ("save_to", "Save path"),
            Command::CreateFile(_) => ("create_file", "Create file"),
            Command::CreateDir(_) => ("create_dir", "Create directory"),
            Command::RenameTo(_) => ("rename_to", "Rename"),
            Command::DeleteTarget(_) => ("delete_target", "Delete"),
            _ => return None,
        })
    }

    fn workspace_navigation_spec(&self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Command::SelectWorkspace => ("select_workspace", "Pick a workspace"),
            Command::SwitchWorkspace(_) => ("switch_workspace", "Switch workspace"),
            Command::NextWorkspace => ("next_workspace", "Switch to the next workspace"),
            Command::PrevWorkspace => ("prev_workspace", "Switch to the previous workspace"),
            _ => return None,
        })
    }

    /// Commands that can be bound to a key or offered in the palette.
    pub fn palette() -> Vec<Command> {
        vec![
            Command::Save,
            Command::SavePrompt,
            Command::OpenPrompt,
            Command::NewPrompt,
            Command::NewScratch,
            Command::CloseDocument,
            Command::ToggleExplorer,
            Command::Messages,
            Command::Help,
            Command::Mcp,
            Command::QuickOpen,
            Command::Palette,
            Command::Messages,
            Command::GoToLinePrompt,
            Command::FindPrompt,
            Command::FindInFiles,
            Command::ReplacePrompt,
            Command::ReplaceInFiles,
            Command::NextDocument,
            Command::PrevDocument,
            Command::NextMatch,
            Command::Undo,
            Command::Redo,
            Command::SelectAll,
            Command::Copy,
            Command::Cut,
            Command::Paste,
            Command::DuplicateLine,
            Command::MoveLinesUp,
            Command::MoveLinesDown,
            Command::DeleteLine,
            Command::ToggleComment,
            Command::ResolveConflictOurs,
            Command::ResolveConflictTheirs,
            Command::ResolveConflictBoth,
            Command::ToggleFold,
            Command::GoToSymbol,
            Command::AddCursorBelow,
            Command::AddCursorAbove,
            Command::ClearCursors,
            Command::ToggleWordWrap,
            Command::LineEndingsLf,
            Command::LineEndingsCrlf,
            Command::LspCompletion,
            Command::LspHover,
            Command::LspDefinition,
            Command::LspInstall,
            Command::LspUninstall,
            Command::McpAdd,
            Command::McpResources,
            Command::McpResourceTemplates,
            Command::McpPrompts,
            Command::AiChat,
            Command::AiPanel,
            Command::AiClear,
            Command::AiCopy,
            Command::AiWider,
            Command::AiNarrower,
            Command::AiScrollLeft,
            Command::AiScrollRight,
            Command::AiAttachFile,
            Command::AiAttachSelection,
            Command::AiClearContext,
            Command::AiUndoEdit,
            Command::AiBackground,
            Command::AiObjectiveAdd,
            Command::AiObjectives,
            Command::AiQueueObjectives,
            Command::AiObjectiveLink,
            Command::AiIndexProject,
            Command::AiContextFiles,
            Command::AiRules,
            Command::AiTrace,
            Command::GitStatus,
            Command::GitDiff,
            Command::GitLog,
            Command::GitPull,
            Command::GitPush,
            Command::GitConflicts,
            Command::GitStage,
            Command::GitUnstage,
            Command::GitStageHunk,
            Command::GitUnstageHunk,
            Command::GitCommit,
            Command::SplitLeftRight,
            Command::SplitTopBottom,
            Command::ClosePane,
            Command::ToggleFullscreen,
            Command::ToggleTabbed,
            Command::NextPaneTab,
            Command::PrevPaneTab,
            Command::ToggleTerminal,
            Command::TerminalNew,
            Command::TerminalClose,
            Command::NextTerminal,
            Command::PrevTerminal,
            Command::TerminalRename,
            Command::SelectTheme,
            Command::SelectAiProvider,
            Command::McpUnsubscribe,
            Command::AiSemanticIndex,
            Command::AiSemanticStatus,
            Command::SelectWorkspace,
            Command::MovePane(Direction::Left),
            Command::MovePane(Direction::Right),
            Command::MovePane(Direction::Up),
            Command::MovePane(Direction::Down),
            Command::ToggleMoveMode,
            Command::ExplorerUp,
            Command::ExplorerDown,
            Command::ExplorerSelect,
            Command::ExplorerExpand,
            Command::ExplorerCollapse,
            Command::ExplorerNewFile,
            Command::ExplorerNewDir,
            Command::ExplorerRename,
            Command::ExplorerDelete,
            Command::ExplorerRefresh,
            Command::ExplorerToggleHidden,
            Command::DuplicateFile,
            Command::CopyPath,
            Command::RevealInTerminal,
            Command::NewWorkspace,
            Command::SetWorkspaceRoot,
            Command::SetWorkspaceTerminalDir,
            Command::RenameWorkspace,
            Command::CloseWorkspace,
            Command::NextWorkspace,
            Command::PrevWorkspace,
            Command::SelectWorkspace,
            Command::Quit,
        ]
    }

    pub fn from_name(name: &str) -> Option<Command> {
        if let Some(index) = name.strip_prefix("switch_workspace_")
            && let Ok(index) = index.parse::<usize>()
            && index > 0
        {
            return Some(Command::SwitchWorkspace(index - 1));
        }
        Command::palette()
            .into_iter()
            .find(|command| command.name() == name)
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Left => "move_pane_left",
        Direction::Right => "move_pane_right",
        Direction::Up => "move_pane_up",
        Direction::Down => "move_pane_down",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_command() -> Vec<Command> {
        vec![
            Command::Quit,
            Command::Save,
            Command::SavePrompt,
            Command::OpenPrompt,
            Command::NewPrompt,
            Command::NewScratch,
            Command::CloseDocument,
            Command::ToggleExplorer,
            Command::Messages,
            Command::Help,
            Command::Mcp,
            Command::QuickOpen,
            Command::Palette,
            Command::Messages,
            Command::GoToLinePrompt,
            Command::FindPrompt,
            Command::FindInFiles,
            Command::ReplacePrompt,
            Command::ReplaceInFiles,
            Command::NextDocument,
            Command::PrevDocument,
            Command::NextMatch,
            Command::Undo,
            Command::Redo,
            Command::SelectAll,
            Command::Copy,
            Command::Cut,
            Command::Paste,
            Command::DuplicateLine,
            Command::MoveLinesUp,
            Command::MoveLinesDown,
            Command::DeleteLine,
            Command::ToggleComment,
            Command::ResolveConflictOurs,
            Command::ResolveConflictTheirs,
            Command::ResolveConflictBoth,
            Command::ToggleFold,
            Command::GoToSymbol,
            Command::AddCursorBelow,
            Command::AddCursorAbove,
            Command::ClearCursors,
            Command::ToggleWordWrap,
            Command::LineEndingsLf,
            Command::LineEndingsCrlf,
            Command::LspCompletion,
            Command::LspHover,
            Command::LspDefinition,
            Command::LspInstall,
            Command::LspUninstall,
            Command::McpAdd,
            Command::McpResources,
            Command::McpResourceTemplates,
            Command::McpPrompts,
            Command::AiChat,
            Command::AiPanel,
            Command::AiClear,
            Command::AiCopy,
            Command::AiWider,
            Command::AiNarrower,
            Command::AiScrollLeft,
            Command::AiScrollRight,
            Command::AiAttachFile,
            Command::AiAttachSelection,
            Command::AiClearContext,
            Command::AiUndoEdit,
            Command::AiBackground,
            Command::AiObjectiveAdd,
            Command::AiObjectives,
            Command::AiQueueObjectives,
            Command::AiObjectiveLink,
            Command::AiIndexProject,
            Command::AiContextFiles,
            Command::AiRules,
            Command::AiTrace,
            Command::GitStatus,
            Command::GitDiff,
            Command::GitLog,
            Command::GitPull,
            Command::GitPush,
            Command::GitConflicts,
            Command::GitStage,
            Command::GitUnstage,
            Command::GitStageHunk,
            Command::GitUnstageHunk,
            Command::GitCommit,
            Command::InsertText("x".to_string()),
            Command::InsertTab,
            Command::InsertNewline,
            Command::Backspace,
            Command::DeleteForward,
            Command::MoveCursor(Move::Left),
            Command::ExtendSelection(Move::Right),
            Command::FocusPane(Direction::Left),
            Command::MovePane(Direction::Left),
            Command::MovePane(Direction::Right),
            Command::MovePane(Direction::Up),
            Command::MovePane(Direction::Down),
            Command::ToggleMoveMode,
            Command::ExplorerUp,
            Command::ExplorerDown,
            Command::ExplorerSelect,
            Command::ExplorerExpand,
            Command::ExplorerCollapse,
            Command::ExplorerNewFile,
            Command::ExplorerNewDir,
            Command::ExplorerRename,
            Command::ExplorerDelete,
            Command::ExplorerRefresh,
            Command::ExplorerToggleHidden,
            Command::DuplicateFile,
            Command::CopyPath,
            Command::RevealInTerminal,
            Command::SplitLeftRight,
            Command::SplitTopBottom,
            Command::ClosePane,
            Command::ToggleFullscreen,
            Command::ToggleTabbed,
            Command::NextPaneTab,
            Command::PrevPaneTab,
            Command::ToggleTerminal,
            Command::TerminalNew,
            Command::TerminalClose,
            Command::NextTerminal,
            Command::PrevTerminal,
            Command::TerminalRename,
            Command::SelectTheme,
            Command::SelectAiProvider,
            Command::McpUnsubscribe,
            Command::AiSemanticIndex,
            Command::AiSemanticStatus,
            Command::NewWorkspace,
            Command::SetWorkspaceRoot,
            Command::SetWorkspaceTerminalDir,
            Command::RenameWorkspace,
            Command::CloseWorkspace,
            Command::SwitchWorkspace(0),
            Command::NextWorkspace,
            Command::PrevWorkspace,
            Command::SaveTo(PathBuf::from("/tmp/x")),
            Command::CreateFile(PathBuf::from("/tmp/x")),
            Command::CreateDir(PathBuf::from("/tmp/x")),
            Command::RenameTo(PathBuf::from("x")),
            Command::DeleteTarget(PathBuf::from("/tmp/x")),
        ]
    }

    #[test]
    fn palette_names_are_concise() {
        let names: Vec<&str> = Command::palette().iter().map(Command::name).collect();
        assert!(names.iter().all(|name| !name.contains("toggle")));
        assert!(!names.contains(&"next_theme"));
        assert!(!names.contains(&"prev_theme"));
        assert!(names.contains(&"select_theme"));
        assert!(names.contains(&"select_workspace"));
        assert!(names.contains(&"lsp_uninstall"));
        assert!(names.contains(&"mcp"));
        assert!(
            !names
                .iter()
                .any(|name| name.starts_with("switch_workspace_"))
        );
    }

    #[test]
    fn every_command_has_a_specification() {
        for command in every_command() {
            assert_ne!(command.name(), "unknown", "{command:?}");
            assert_ne!(command.description(), "Unknown command", "{command:?}");
        }
    }

    #[test]
    fn indexed_workspace_names_resolve() {
        assert_eq!(
            Command::from_name("switch_workspace_2"),
            Some(Command::SwitchWorkspace(1))
        );
        assert_eq!(Command::from_name("switch_workspace_0"), None);
        assert_eq!(Command::from_name("switch_workspace_x"), None);
    }

    #[test]
    fn palette_names_resolve_back_to_their_command() {
        for command in Command::palette() {
            assert_eq!(
                Command::from_name(command.name()),
                Some(command.clone()),
                "{}",
                command.name()
            );
        }
    }
}
