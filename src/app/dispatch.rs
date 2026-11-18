//! Command dispatch. Every state mutation enters here.

use crate::command::Command;
use crate::error::Result;
use crate::input::PromptKind;
use crate::workspace::SplitAxis;

use super::App;

impl App {
    pub fn dispatch(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Quit => self.request_quit()?,
            Command::Save => self.save()?,
            Command::SavePrompt => self.open_prompt(PromptKind::SaveAs),
            Command::OpenPrompt | Command::QuickOpen => self.open_prompt(PromptKind::QuickOpen),
            Command::NewPrompt => self.new_buffer()?,
            Command::CloseDocument => self.close_document()?,
            Command::ToggleExplorer => self.toggle_explorer(),
            Command::Palette => self.open_prompt(PromptKind::CommandPalette),
            Command::GoToLinePrompt => self.open_prompt(PromptKind::GoToLine),
            Command::FindPrompt => self.open_prompt(PromptKind::Find),
            Command::ReplacePrompt => self.open_prompt(PromptKind::Replace),
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
            Command::InsertText(text) => self.insert_text(&text)?,
            Command::InsertNewline => self.insert_newline()?,
            Command::Backspace => self.delete_backward()?,
            Command::DeleteForward => self.delete_forward()?,
            Command::MoveCursor(movement) => self.move_cursor(movement, false)?,
            Command::ExtendSelection(movement) => self.move_cursor(movement, true)?,
            Command::FocusPane(direction) => self.focus_direction(direction)?,
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
            Command::SplitLeftRight => self.split_pane(SplitAxis::LeftRight)?,
            Command::SplitTopBottom => self.split_pane(SplitAxis::TopBottom)?,
            Command::ToggleTerminal => self.toggle_shell(),
            Command::SelectTheme => self.open_prompt(PromptKind::Theme),
            Command::NextTheme => self.cycle_theme(1),
            Command::PrevTheme => self.cycle_theme(-1),
            Command::ClosePane => self.close_pane()?,
            Command::ToggleFullscreen => self.toggle_fullscreen(),
            Command::ToggleTabbed => self.toggle_tabbed(),
            Command::NextPaneTab => self.cycle_pane_tab(1),
            Command::PrevPaneTab => self.cycle_pane_tab(-1),
            Command::MovePane(direction) => self.move_pane(direction)?,
            Command::ToggleMoveMode => self.toggle_move_mode(),
            Command::NewWorkspace => self.new_workspace()?,
            Command::CloseWorkspace => self.close_workspace()?,
            Command::SwitchWorkspace(index) => self.switch_workspace(index)?,
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
        }
        Ok(())
    }
}
