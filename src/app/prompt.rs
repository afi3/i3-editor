//! Prompt input and submission.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::Command;
use crate::error::{Error, Result};
use crate::input::{KeyCombo, PromptKind, PromptState};
use crate::state::InputMode;

use super::App;

impl App {
    pub(crate) fn handle_prompt_key(&mut self, key: KeyEvent, combo: KeyCombo) {
        let _ = key;
        match combo.code {
            KeyCode::Esc => {
                self.state.mode = InputMode::Normal;
                return;
            }
            KeyCode::Enter => {
                let result = self.submit_prompt();
                self.finish(result);
                return;
            }
            KeyCode::Tab => {
                if let InputMode::Prompt(prompt) = &mut self.state.mode {
                    prompt.next_field();
                }
                return;
            }
            _ => {}
        }

        let InputMode::Prompt(prompt) = &mut self.state.mode else {
            return;
        };
        let lists = prompt.kind.lists_candidates();
        match combo.code {
            KeyCode::Up if lists => prompt.move_selection(-1),
            KeyCode::Down if lists => prompt.move_selection(1),
            KeyCode::Left => prompt.cursor_left(),
            KeyCode::Right => prompt.cursor_right(),
            KeyCode::Home => prompt.cursor = 0,
            KeyCode::End => prompt.cursor = prompt.input().chars().count(),
            KeyCode::Backspace => prompt.backspace(),
            KeyCode::Delete => prompt.delete(),
            KeyCode::Char(ch)
                if !combo
                    .mods
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                prompt.insert_char(ch)
            }
            _ => {}
        }
    }

    pub(crate) fn submit_prompt(&mut self) -> Result<()> {
        let InputMode::Prompt(prompt) = std::mem::replace(&mut self.state.mode, InputMode::Normal)
        else {
            return Ok(());
        };
        let root = self.state.explorer.root.clone();

        match prompt.kind {
            PromptKind::CommandPalette => {
                let Some(name) = prompt.selected_candidate() else {
                    return Ok(());
                };
                match Command::from_name(name) {
                    Some(command) => self.dispatch(command),
                    None => {
                        self.notify(format!("unknown command {name}"));
                        Ok(())
                    }
                }
            }
            PromptKind::QuickOpen => {
                let Some(relative) = prompt.selected_candidate() else {
                    return Ok(());
                };
                self.open_document(root.join(relative))
            }
            PromptKind::Find => self.start_search(prompt.fields[0].clone()),
            PromptKind::Replace => self.replace_all(&prompt.fields[0], &prompt.fields[1]),
            PromptKind::GoToLine => {
                let line: usize = prompt.fields[0]
                    .trim()
                    .parse()
                    .map_err(|_| Error::InvalidPath(PathBuf::from(&prompt.fields[0])))?;
                self.goto_line(line.saturating_sub(1))
            }
            PromptKind::SaveAs => {
                let path = resolve(&root, &prompt.fields[0])?;
                self.dispatch(Command::SaveTo(path))
            }
            PromptKind::NewFile => {
                let path = resolve(&self.state.explorer.target_dir(), &prompt.fields[0])?;
                self.dispatch(Command::CreateFile(path))
            }
            PromptKind::NewDir => {
                let path = resolve(&self.state.explorer.target_dir(), &prompt.fields[0])?;
                self.dispatch(Command::CreateDir(path))
            }
            PromptKind::Rename => {
                let name = prompt.fields[0].clone();
                self.dispatch(Command::RenameTo(PathBuf::from(name)))
            }
            PromptKind::Delete => match prompt.target.clone() {
                Some(path) => self.dispatch(Command::DeleteTarget(path)),
                None => Ok(()),
            },
            PromptKind::Discard => {
                if let Some(id) = prompt.document {
                    self.force_close_document(id);
                }
                Ok(())
            }
        }
    }

    pub(crate) fn open_prompt(&mut self, kind: PromptKind) {
        let mut prompt = PromptState::new(kind);
        match kind {
            PromptKind::CommandPalette => {
                prompt.set_candidates(
                    Command::palette()
                        .iter()
                        .map(|command| command.name().to_string())
                        .collect(),
                );
            }
            PromptKind::QuickOpen => {
                prompt.set_candidates(self.quick_open_candidates());
            }
            PromptKind::Find => {
                if let Some(search) = &self.state.search {
                    prompt = prompt.with_field(search.query.clone());
                }
            }
            PromptKind::Replace => {
                if let Some(search) = &self.state.search {
                    prompt = prompt.with_field(search.query.clone());
                }
            }
            PromptKind::SaveAs => {
                if let Some(document) = self.state.active_document() {
                    if let Some(path) = &document.path {
                        prompt = prompt.with_field(path.to_string_lossy().into_owned());
                    } else {
                        let name = document.display_name();
                        let name = if name == "[No Name]" {
                            "untitled.txt".to_string()
                        } else {
                            name
                        };
                        prompt = prompt.with_field(
                            self.state
                                .explorer
                                .target_dir()
                                .join(name)
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
            }
            PromptKind::Rename => {
                if let Some(row) = self.state.explorer.selected_row() {
                    prompt = prompt.with_field(row.name.clone());
                }
            }
            PromptKind::NewFile | PromptKind::NewDir => {
                prompt = prompt.with_field("");
            }
            PromptKind::Delete => {
                prompt.target = self
                    .state
                    .explorer
                    .selected_row()
                    .map(|row| row.path.clone());
            }
            PromptKind::Discard => {}
            PromptKind::GoToLine => {
                if let (Some(pane), Some(document)) =
                    (self.state.focused_pane(), self.state.active_document())
                {
                    prompt =
                        prompt.with_field((document.line_of_char(pane.cursor) + 1).to_string());
                }
            }
        }
        self.state.mode = InputMode::Prompt(prompt);
    }

    fn quick_open_candidates(&self) -> Vec<String> {
        let root = self.state.explorer.root.clone();
        let mut out = Vec::new();
        let walker = ignore::WalkBuilder::new(&root)
            .hidden(!self.state.explorer.show_hidden)
            .git_ignore(true)
            .build();
        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Ok(relative) = path.strip_prefix(&root) {
                out.push(relative.to_string_lossy().into_owned());
            }
            if out.len() >= 5000 {
                break;
            }
        }
        out.sort();
        out
    }
}

fn resolve(base: &Path, text: &str) -> Result<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::InvalidPath(PathBuf::new()));
    }
    let path = PathBuf::from(text);
    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}
