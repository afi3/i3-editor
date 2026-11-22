//! Prompt input and submission.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::command::Command;
use crate::error::{Error, Result};
use crate::input::{KeyCombo, PromptGroup, PromptKind, PromptState};
use crate::state::InputMode;

use super::App;

impl App {
    pub(crate) fn handle_prompt_key(&mut self, combo: KeyCombo) {
        if combo.code == KeyCode::Char('c') && combo.mods.contains(KeyModifiers::CONTROL) {
            self.cancel_prompt();
            return;
        }
        match combo.code {
            KeyCode::Esc => {
                self.cancel_prompt();
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
        if self.handle_find_arrow(combo) {
            return;
        }
        if self.edit_prompt(combo) {
            self.refresh_prompt_preview();
        }
    }

    /// Find uses Up/Down for its match list instead of prompt candidates.
    fn handle_find_arrow(&mut self, combo: KeyCombo) -> bool {
        let InputMode::Prompt(prompt) = &self.state.mode else {
            return false;
        };
        if prompt.kind != PromptKind::Find {
            return false;
        }
        match combo.code {
            KeyCode::Up => {
                self.move_search_selection(-1);
                true
            }
            KeyCode::Down => {
                self.move_search_selection(1);
                true
            }
            _ => false,
        }
    }

    /// Apply an editing key to the prompt field. Returns whether the query
    /// changed, so live searches can refresh.
    fn edit_prompt(&mut self, combo: KeyCombo) -> bool {
        let InputMode::Prompt(prompt) = &mut self.state.mode else {
            return false;
        };
        let lists = prompt.kind.lists_candidates();
        let mut edited = false;
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
        edited
    }

    pub(crate) fn refresh_prompt_preview(&mut self) {
        let InputMode::Prompt(prompt) = &self.state.mode else {
            return;
        };
        match prompt.kind {
            PromptKind::Find => {
                let query = prompt.input().to_string();
                self.preview_search(query);
            }
            PromptKind::FindInFiles => {
                let query = prompt.input().to_string();
                self.start_project_search(query);
            }
            _ => {}
        }
    }

    /// Close the prompt without submitting. A running project scan is stopped.
    pub(crate) fn cancel_prompt(&mut self) {
        if matches!(&self.state.mode, InputMode::Prompt(prompt) if prompt.kind == PromptKind::FindInFiles)
        {
            self.stop_project_search();
        }
        self.state.mode = InputMode::Normal;
    }

    pub(crate) fn submit_prompt(&mut self) -> Result<()> {
        let InputMode::Prompt(prompt) = std::mem::replace(&mut self.state.mode, InputMode::Normal)
        else {
            return Ok(());
        };
        match prompt.kind.group() {
            PromptGroup::Query => self.submit_query_prompt(prompt),
            PromptGroup::File => self.submit_file_prompt(prompt),
            PromptGroup::Session => self.submit_session_prompt(prompt),
        }
    }

    fn submit_query_prompt(&mut self, prompt: PromptState) -> Result<()> {
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
            PromptKind::Messages => Ok(()),
            PromptKind::Find => self.commit_search(prompt.fields[0].clone()),
            PromptKind::FindInFiles => {
                let selected = prompt.filtered.get(prompt.selected).copied();
                let found = selected.and_then(|index| {
                    self.state
                        .project_search
                        .as_ref()
                        .and_then(|search| search.results.get(index))
                        .cloned()
                });
                self.stop_project_search();
                match found {
                    Some(found) => self.jump_to_project_match(found),
                    None if prompt.fields[0].is_empty() => {
                        self.notify("nothing to search");
                        Ok(())
                    }
                    None => {
                        self.notify(format!("no matches for {}", prompt.fields[0]));
                        Ok(())
                    }
                }
            }
            PromptKind::Replace => self.replace_all(&prompt.fields[0], &prompt.fields[1]),
            PromptKind::GoToLine => {
                let line: usize = prompt.fields[0]
                    .trim()
                    .parse()
                    .map_err(|_| Error::InvalidPath(PathBuf::from(&prompt.fields[0])))?;
                self.goto_line(line.saturating_sub(1))
            }
            _ => Ok(()),
        }
    }

    fn submit_file_prompt(&mut self, prompt: PromptState) -> Result<()> {
        let root = self.state.explorer.root.clone();
        match prompt.kind {
            PromptKind::OpenPath => {
                let path = resolve(&root, &prompt.fields[0])?;
                self.open_document(path)
            }
            PromptKind::QuickOpen => {
                let Some(relative) = prompt.selected_candidate() else {
                    return Ok(());
                };
                self.open_document(root.join(relative))
            }
            PromptKind::SaveAs => self.submit_save_as(&root, &prompt.fields[0]),
            PromptKind::Overwrite => match prompt.target.clone() {
                Some(path) => self.dispatch(Command::SaveTo(path)),
                None => Ok(()),
            },
            PromptKind::ExternalChange => self.submit_external_change(&prompt),
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
            _ => Ok(()),
        }
    }

    fn submit_save_as(&mut self, root: &Path, field: &str) -> Result<()> {
        let path = resolve(root, field)?;
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

    fn submit_external_change(&mut self, prompt: &PromptState) -> Result<()> {
        match prompt.selected_candidate() {
            Some("Overwrite with the buffer") => match prompt.target.clone() {
                Some(path) => self.force_save_to(path),
                None => Ok(()),
            },
            Some("Reload from disk") => match prompt.document {
                Some(id) => self.reload_document(id),
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn submit_session_prompt(&mut self, prompt: PromptState) -> Result<()> {
        match prompt.kind {
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
            _ => Ok(()),
        }
    }

    pub(crate) fn open_prompt(&mut self, kind: PromptKind) {
        let mut prompt = PromptState::new(kind);
        let filled = match kind.group() {
            PromptGroup::Query => self.fill_query_prompt(kind, &mut prompt),
            PromptGroup::File => self.fill_file_prompt(kind, &mut prompt),
            PromptGroup::Session => self.fill_session_prompt(kind, &mut prompt),
        };
        if !filled {
            return;
        }
        let project_query = (kind == PromptKind::FindInFiles).then(|| prompt.input().to_string());
        self.state.mode = InputMode::Prompt(prompt);
        if let Some(query) = project_query {
            self.start_project_search(query);
        }
    }

    fn fill_query_prompt(&mut self, kind: PromptKind, prompt: &mut PromptState) -> bool {
        match kind {
            PromptKind::CommandPalette => {
                let mut candidates: Vec<String> = Command::palette()
                    .iter()
                    .map(|command| command.name().to_string())
                    .collect();
                for index in 1..=self.state.workspaces.workspaces.len() {
                    candidates.push(format!("switch_workspace_{index}"));
                }
                prompt.set_candidates(candidates);
            }
            PromptKind::Messages => {
                if self.state.warnings.is_empty() {
                    self.notify("no messages");
                    return false;
                }
                prompt.set_candidates(self.state.warnings.clone());
            }
            PromptKind::Find | PromptKind::Replace => {
                if let Some(search) = &self.state.search {
                    prompt.set_input(search.query.clone());
                }
            }
            PromptKind::FindInFiles => {
                if let Some(search) = &self.state.project_search {
                    prompt.set_input(search.query.clone());
                }
            }
            PromptKind::GoToLine => {
                if let (Some(pane), Some(document)) =
                    (self.state.focused_pane(), self.state.active_document())
                {
                    prompt.set_input((document.line_of_char(pane.cursor) + 1).to_string());
                }
            }
            _ => {}
        }
        true
    }

    fn fill_file_prompt(&mut self, kind: PromptKind, prompt: &mut PromptState) -> bool {
        match kind {
            PromptKind::QuickOpen => prompt.set_candidates(self.quick_open_candidates()),
            PromptKind::SaveAs => {
                if let Some(document) = self.state.active_document() {
                    if let Some(path) = &document.path {
                        prompt.set_input(path.to_string_lossy().into_owned());
                    } else {
                        let name = document.display_name();
                        let name = if name == "[No Name]" {
                            "untitled.txt".to_string()
                        } else {
                            name
                        };
                        prompt.set_input(
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
            PromptKind::NewFile | PromptKind::NewDir => prompt.set_input(""),
            PromptKind::Rename => {
                if let Some(row) = self.state.explorer.selected_row() {
                    prompt.set_input(row.name.clone());
                }
            }
            PromptKind::Delete => {
                prompt.target = self
                    .state
                    .explorer
                    .selected_row()
                    .map(|row| row.path.clone());
            }
            _ => {}
        }
        true
    }

    fn fill_session_prompt(&mut self, kind: PromptKind, prompt: &mut PromptState) -> bool {
        if kind == PromptKind::Theme {
            prompt.set_candidates(crate::theme::Theme::names(&self.state.themes_dir));
            prompt.select_candidate(&self.state.config.appearance.theme);
        }
        true
    }

    pub(crate) fn quick_open_candidates(&mut self) -> Vec<String> {
        // Without a watcher the list cannot be invalidated, so keep rescanning.
        if self.watcher.is_none() {
            return self.scan_quick_open();
        }
        let revision = self.state.explorer.revision;
        if let Some((cached, files)) = &self.quick_open_files
            && *cached == revision
        {
            return files.clone();
        }
        let files = self.scan_quick_open();
        self.quick_open_files = Some((revision, files.clone()));
        files
    }

    fn scan_quick_open(&self) -> Vec<String> {
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
