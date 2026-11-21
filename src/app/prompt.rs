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

        if let InputMode::Prompt(prompt) = &self.state.mode
            && prompt.kind == PromptKind::Find
        {
            match combo.code {
                KeyCode::Up => {
                    self.move_search_selection(-1);
                    return;
                }
                KeyCode::Down => {
                    self.move_search_selection(1);
                    return;
                }
                _ => {}
            }
        }

        let mut edited = false;
        {
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
                KeyCode::Backspace => {
                    prompt.backspace();
                    edited = true;
                }
                KeyCode::Delete => {
                    prompt.delete();
                    edited = true;
                }
                KeyCode::Char(ch)
                    if !combo
                        .mods
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    prompt.insert_char(ch);
                    edited = true;
                }
                _ => {}
            }
        }

        if edited
            && let InputMode::Prompt(prompt) = &self.state.mode
            && prompt.kind == PromptKind::Find
        {
            let query = prompt.input().to_string();
            self.preview_search(query);
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
            PromptKind::Find => self.commit_search(prompt.fields[0].clone()),
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
                let current = self
                    .state
                    .active_document()
                    .and_then(|document| document.path.clone());
                if path.exists() && current.as_deref() != Some(path.as_path()) {
                    self.open_prompt(PromptKind::Overwrite);
                    if let InputMode::Prompt(prompt) = &mut self.state.mode {
                        prompt.target = Some(path);
                    }
                    Ok(())
                } else {
                    self.dispatch(Command::SaveTo(path))
                }
            }
            PromptKind::Overwrite => match prompt.target.clone() {
                Some(path) => self.dispatch(Command::SaveTo(path)),
                None => Ok(()),
            },
            PromptKind::ExternalChange => match prompt.selected_candidate() {
                Some("Overwrite with the buffer") => match prompt.target.clone() {
                    Some(path) => self.force_save_to(path),
                    None => Ok(()),
                },
                Some("Reload from disk") => match prompt.document {
                    Some(id) => self.reload_document(id),
                    None => Ok(()),
                },
                _ => Ok(()),
            },
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
            PromptKind::Theme => {
                let Some(name) = prompt.selected_candidate().map(str::to_string) else {
                    return Ok(());
                };
                self.apply_theme(&name);
                Ok(())
            }
            PromptKind::QuitDirty => {
                match prompt.selected_candidate() {
                    Some("Save all and quit") => {
                        self.save_all()?;
                        self.state.should_quit = true;
                    }
                    Some("Discard all and quit") => self.state.should_quit = true,
                    _ => {}
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
            PromptKind::Discard | PromptKind::Overwrite | PromptKind::ExternalChange => {}
            PromptKind::Theme => {
                prompt.set_candidates(crate::theme::Theme::names(&self.state.themes_dir));
                prompt.select_candidate(&self.state.config.appearance.theme);
            }
            PromptKind::QuitDirty => {}
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
        let show_all = self.state.explorer.show_hidden;
        let walker = ignore::WalkBuilder::new(&root)
            .hidden(!show_all)
            .git_ignore(!show_all)
            .git_global(!show_all)
            .git_exclude(!show_all)
            .ignore(!show_all)
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
