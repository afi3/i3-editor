//! Command dispatch. Every state mutation enters here.

use crate::command::Command;
use crate::error::{Error, Result};
use crate::input::PromptKind;
use crate::workspace::SplitAxis;

use super::App;

impl App {
    /// Route a command to its category. Each category takes ownership and hands
    /// the command back when it does not recognize it, so payloads never clone.
    pub fn dispatch(&mut self, command: Command) -> Result<()> {
        let Some(command) = self.dispatch_document(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_prompt(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_navigation(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_code(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_conflicts(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_git(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_text(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_file_ops(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_explorer(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_layout(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_terminal(command)? else {
            return Ok(());
        };
        let Some(command) = self.dispatch_workspace(command)? else {
            return Ok(());
        };
        Err(Error::UnhandledCommand(command.name()))
    }

    fn dispatch_document(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::Quit => self.request_quit()?,
            Command::Save => self.save()?,
            Command::NewScratch => self.new_scratch()?,
            Command::CloseDocument => self.close_document()?,
            Command::ToggleExplorer => self.toggle_explorer(),
            Command::Help => self.open_help(),
            Command::Mcp => self.open_mcp(),
            Command::McpAdd => self.open_mcp_add(),
            Command::AiChat => self.open_ai(),
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_prompt(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::SavePrompt => self.open_prompt(PromptKind::SaveAs),
            Command::OpenPrompt => self.open_prompt(PromptKind::OpenPath),
            Command::QuickOpen => self.open_prompt(PromptKind::QuickOpen),
            Command::NewPrompt => self.open_prompt(PromptKind::NewFile),
            Command::Palette => self.open_prompt(PromptKind::CommandPalette),
            Command::Messages => self.open_prompt(PromptKind::Messages),
            Command::GoToLinePrompt => self.open_prompt(PromptKind::GoToLine),
            Command::FindPrompt => self.open_prompt(PromptKind::Find),
            Command::FindInFiles => self.open_prompt(PromptKind::FindInFiles),
            Command::ReplacePrompt => self.open_prompt(PromptKind::Replace),
            Command::ReplaceInFiles => self.open_prompt(PromptKind::ReplaceInFiles),
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_navigation(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::NextDocument => self.cycle_document(1)?,
            Command::PrevDocument => self.cycle_document(-1)?,
            Command::NextMatch => self.next_match()?,
            Command::Undo => self.undo()?,
            Command::Redo => self.redo()?,
            Command::SelectAll => self.select_all()?,
            Command::Copy => self.copy_selection(false)?,
            Command::Cut => self.copy_selection(true)?,
            Command::Paste => {
                let text = self.state.clipboard.clone();
                if !text.is_empty() {
                    self.insert_text(&text)?;
                }
            }
            Command::DuplicateLine => self.duplicate_line()?,
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_code(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::AddCursorBelow => self.add_cursor(1)?,
            Command::AddCursorAbove => self.add_cursor(-1)?,
            Command::ClearCursors => self.clear_cursors(),
            Command::ToggleWordWrap => self.toggle_word_wrap(),
            Command::LineEndingsLf => {
                self.convert_line_endings(crate::editor::document::Eol::Lf)?
            }
            Command::LineEndingsCrlf => {
                self.convert_line_endings(crate::editor::document::Eol::Crlf)?
            }
            Command::GoToSymbol => self.open_prompt(PromptKind::Symbols),
            Command::LspCompletion => self.lsp_completion()?,
            Command::LspHover => self.lsp_hover()?,
            Command::LspDefinition => self.lsp_definition()?,
            Command::LspInstall => self.lsp_install()?,
            Command::LspUninstall => self.lsp_uninstall()?,
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_git(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::GitStatus => self.git_status()?,
            Command::GitDiff => self.git_diff()?,
            Command::GitLog => self.git_log()?,
            Command::GitPull => self.git_pull()?,
            Command::GitPush => self.git_push()?,
            Command::GitConflicts => self.git_conflicts()?,
            Command::GitStage => self.git_stage()?,
            Command::GitUnstage => self.git_unstage()?,
            Command::GitStageHunk => self.git_hunk_picker(true)?,
            Command::GitUnstageHunk => self.git_hunk_picker(false)?,
            Command::GitCommit => self.git_commit_prompt()?,
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_conflicts(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::ResolveConflictOurs => self.resolve_conflict(crate::conflict::Choice::Ours)?,
            Command::ResolveConflictTheirs => {
                self.resolve_conflict(crate::conflict::Choice::Theirs)?
            }
            Command::ResolveConflictBoth => self.resolve_conflict(crate::conflict::Choice::Both)?,
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_text(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::InsertText(text) => self.insert_text(&text)?,
            Command::InsertTab => self.insert_tab()?,
            Command::InsertNewline => self.insert_newline()?,
            Command::Backspace => self.delete_backward()?,
            Command::DeleteForward => self.delete_forward()?,
            Command::MoveCursor(movement) => self.move_cursor(movement, false)?,
            Command::ExtendSelection(movement) => self.move_cursor(movement, true)?,
            Command::MoveLinesUp => self.move_lines(-1)?,
            Command::MoveLinesDown => self.move_lines(1)?,
            Command::DeleteLine => self.delete_line()?,
            Command::ToggleComment => self.toggle_comment()?,
            Command::ToggleFold => self.toggle_fold()?,
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_file_ops(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::DuplicateFile => self.duplicate_file()?,
            Command::CopyPath => self.copy_path()?,
            Command::RevealInTerminal => self.reveal_in_terminal()?,
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_explorer(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::ExplorerUp => self.state.explorer.move_selection(-1),
            Command::ExplorerDown => self.state.explorer.move_selection(1),
            Command::ExplorerSelect => self.explorer_open()?,
            Command::ExplorerExpand => self.state.explorer.expand_selected(),
            Command::ExplorerCollapse => self.state.explorer.collapse_selected(),
            Command::ExplorerNewFile => self.open_prompt(PromptKind::NewFile),
            Command::ExplorerNewDir => self.open_prompt(PromptKind::NewDir),
            Command::ExplorerRename => self.open_prompt(PromptKind::Rename),
            Command::ExplorerDelete => self.open_prompt(PromptKind::Delete),
            Command::ExplorerRefresh => self.state.explorer.refresh(),
            Command::ExplorerToggleHidden => self.state.explorer.toggle_hidden(),
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_layout(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::SplitLeftRight => self.split_pane(SplitAxis::LeftRight)?,
            Command::SplitTopBottom => self.split_pane(SplitAxis::TopBottom)?,
            Command::ClosePane => self.close_pane()?,
            Command::ToggleFullscreen => self.toggle_fullscreen(),
            Command::ToggleTabbed => self.toggle_tabbed(),
            Command::NextPaneTab => self.cycle_container_tab(1),
            Command::PrevPaneTab => self.cycle_container_tab(-1),
            Command::FocusPane(direction) => self.focus_direction(direction)?,
            Command::MovePane(direction) => self.move_pane(direction)?,
            Command::ToggleMoveMode => self.toggle_move_mode(),
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_terminal(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::ToggleTerminal => self.toggle_shell(),
            Command::TerminalNew => self.open_terminal(),
            Command::TerminalClose => self.close_terminal(),
            Command::NextTerminal => self.cycle_terminal(1),
            Command::PrevTerminal => self.cycle_terminal(-1),
            Command::TerminalRename => self.rename_terminal_prompt(),
            Command::SelectTheme => self.open_prompt(PromptKind::Theme),
            Command::SelectWorkspace => self.open_prompt(PromptKind::SelectWorkspace),
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    fn dispatch_workspace(&mut self, command: Command) -> Result<Option<Command>> {
        match command {
            Command::NewWorkspace => self.new_workspace()?,
            Command::SetWorkspaceRoot => self.open_prompt(PromptKind::WorkspaceRoot),
            Command::SetWorkspaceTerminalDir => self.open_prompt(PromptKind::WorkspaceTerminalDir),
            Command::RenameWorkspace => self.open_prompt(PromptKind::RenameWorkspace),
            Command::CloseWorkspace => self.close_workspace()?,
            Command::SwitchWorkspace(index) => self.switch_workspace(index)?,
            Command::NextWorkspace => self.cycle_workspace(1)?,
            Command::PrevWorkspace => self.cycle_workspace(-1)?,
            Command::SaveTo(path) => self.save_to(path)?,
            Command::CreateFile(path) => {
                self.state.explorer.create(&path, false)?;
                self.open_document(path)?;
            }
            Command::CreateDir(path) => {
                self.state.explorer.create(&path, true)?;
            }
            Command::RenameTo(name) => {
                let display = name.to_string_lossy().into_owned();
                let path = self.state.explorer.rename(&display)?;
                self.notify(format!("renamed to {}", path.display()));
            }
            Command::DeleteTarget(path) => {
                self.state.explorer.delete_at(&path)?;
                self.notify(format!("deleted {}", path.display()));
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    /// Tab cycling follows the focus: shell tabs in the panel, document tabs in
    /// a tabbed container.
    fn cycle_container_tab(&mut self, delta: isize) {
        if matches!(self.state.focus, crate::state::Focus::Shell) {
            self.cycle_terminal(delta);
        } else {
            self.cycle_pane_tab(delta);
        }
    }
}
