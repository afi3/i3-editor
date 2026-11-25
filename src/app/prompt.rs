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
        if self.toggle_search_option(combo) {
            return;
        }
        if self.handle_search_history(combo) {
            return;
        }
        match combo.code {
            KeyCode::Esc => {
                self.cancel_prompt();
                return;
            }
            KeyCode::Enter => {
                let replace_file = matches!(
                    &self.state.mode,
                    InputMode::Prompt(prompt)
                        if prompt.kind == PromptKind::ReplaceInFiles
                            && (combo.mods.contains(KeyModifiers::CONTROL) || prompt.active > 0)
                );
                if replace_file {
                    let result = self.replace_selected_project_file();
                    self.finish(result);
                    return;
                }
                if let InputMode::Prompt(prompt) = &mut self.state.mode
                    && prompt.kind == PromptKind::ReplaceInFiles
                {
                    // First Enter moves from the query to the replacement; the
                    // next one is the per-file confirmation.
                    prompt.next_field();
                    return;
                }
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
        if edited {
            prompt.history.index = None;
        }
        edited
    }

    pub(crate) fn refresh_prompt_preview(&mut self) {
        let InputMode::Prompt(prompt) = &self.state.mode else {
            return;
        };
        let query_changed = prompt.active == 0;
        match prompt.kind {
            PromptKind::Find => {
                let query = prompt.fields.first().cloned().unwrap_or_default();
                self.preview_search(query);
            }
            PromptKind::Replace if query_changed => {
                let query = prompt.fields.first().cloned().unwrap_or_default();
                self.preview_search(query);
            }
            PromptKind::FindInFiles | PromptKind::ReplaceInFiles if query_changed => {
                let query = prompt.fields.first().cloned().unwrap_or_default();
                self.start_project_search(query);
            }
            _ => {}
        }
    }

    /// Alt+R/Alt+W/Alt+C toggle search options while a search prompt is open.
    fn toggle_search_option(&mut self, combo: KeyCombo) -> bool {
        if !combo.mods.contains(KeyModifiers::ALT)
            || !matches!(
                &self.state.mode,
                InputMode::Prompt(prompt)
                    if matches!(
                        prompt.kind,
                        PromptKind::Find
                            | PromptKind::Replace
                            | PromptKind::FindInFiles
                            | PromptKind::ReplaceInFiles
                    )
            )
        {
            return false;
        }
        let mut options = self.state.search_options;
        match combo.code {
            KeyCode::Char('r') => options.regex = !options.regex,
            KeyCode::Char('w') => options.whole_word = !options.whole_word,
            KeyCode::Char('c') => options.case_sensitive = !options.case_sensitive,
            _ => return false,
        }
        let (kind, query) = {
            let InputMode::Prompt(prompt) = &self.state.mode else {
                return false;
            };
            (
                prompt.kind,
                prompt.fields.first().cloned().unwrap_or_default(),
            )
        };
        self.state.search_options = options;
        self.search_history.options = options;
        self.save_search_history();
        self.notify(options.summary());
        match kind {
            PromptKind::Find | PromptKind::Replace => self.preview_search(query),
            PromptKind::FindInFiles | PromptKind::ReplaceInFiles => {
                self.start_project_search(query)
            }
            _ => {}
        }
        true
    }

    /// Close the prompt without submitting. A running project scan is stopped.
    pub(crate) fn cancel_prompt(&mut self) {
        if matches!(
            &self.state.mode,
            InputMode::Prompt(prompt) if prompt.kind == PromptKind::McpSampling
        ) {
            self.state.mode = InputMode::Normal;
            if let Some((server, id, _)) = self.sampling_request.take()
                && let Some(manager) = self.mcp.as_mut()
            {
                let _ = manager.respond_error(&server, id, "sampling denied by user");
            }
            return;
        }
        if let InputMode::Prompt(prompt) = &self.state.mode
            && prompt.kind == PromptKind::Recovery
        {
            let id = prompt.document;
            self.state.mode = InputMode::Normal;
            if let Some(id) = id {
                let result = self.resolve_recovery(id, "Keep unsaved buffer");
                self.finish(result);
            }
            return;
        }
        if matches!(
            &self.state.mode,
            InputMode::Prompt(prompt)
                if matches!(
                    prompt.kind,
                    PromptKind::FindInFiles | PromptKind::ReplaceInFiles
                )
        ) {
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

    fn submit_workspace_choice(&mut self, candidate: Option<&str>) -> Result<()> {
        let index = candidate
            .and_then(|candidate| candidate.split_whitespace().next())
            .and_then(|index| index.parse::<usize>().ok())
            .map(|index| index.saturating_sub(1));
        match index {
            Some(index) => self.switch_workspace(index),
            None => Ok(()),
        }
    }

    fn submit_query_prompt(&mut self, prompt: PromptState) -> Result<()> {
        if matches!(
            prompt.kind,
            PromptKind::GitStatus | PromptKind::Conflicts | PromptKind::CommitMessage
        ) {
            return self.submit_git_prompt(prompt);
        }
        if prompt.kind.is_mcp() {
            self.submit_mcp_prompt(prompt);
            return Ok(());
        }
        if prompt.kind == PromptKind::AiBackground {
            self.submit_ai_background(prompt);
            return Ok(());
        }
        if prompt.kind == PromptKind::AiObjectiveAdd {
            self.submit_ai_objective_add(prompt);
            return Ok(());
        }
        if prompt.kind == PromptKind::AiObjectives {
            self.submit_ai_objective(prompt);
            return Ok(());
        }
        if prompt.kind == PromptKind::AiContextFiles {
            self.submit_ai_context_file(prompt);
            return Ok(());
        }
        self.submit_regular_query(prompt)
    }

    fn submit_regular_query(&mut self, prompt: PromptState) -> Result<()> {
        let history = matches!(
            prompt.kind,
            PromptKind::Find
                | PromptKind::FindInFiles
                | PromptKind::Replace
                | PromptKind::ReplaceInFiles
        )
        .then(|| (prompt.kind, prompt.fields[0].clone()));
        let result = match prompt.kind {
            PromptKind::CommandPalette => {
                let Some(name) = prompt.selected_candidate() else {
                    return Ok(());
                };
                self.submit_palette(name)
            }
            PromptKind::SelectWorkspace => {
                self.submit_workspace_choice(prompt.selected_candidate())
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
            PromptKind::Symbols => {
                let selected = prompt.filtered.get(prompt.selected).copied();
                let Some(symbol) = selected
                    .and_then(|index| prompt.symbols.get(index))
                    .cloned()
                else {
                    return Ok(());
                };
                self.goto_symbol(symbol)
            }
            PromptKind::GoToLine => {
                let line: usize = prompt.fields[0]
                    .trim()
                    .parse()
                    .map_err(|_| Error::InvalidPath(PathBuf::from(&prompt.fields[0])))?;
                self.goto_line(line.saturating_sub(1))
            }
            _ => Ok(()),
        };
        if result.is_ok()
            && let Some((kind, query)) = history
        {
            self.remember_search(kind, &query);
        }
        result
    }

    fn submit_palette(&mut self, name: &str) -> Result<()> {
        match Command::from_name(name) {
            Some(command) => self.dispatch(command),
            None if self.run_plugin_command(name) => Ok(()),
            None => {
                self.notify(format!("unknown command {name}"));
                Ok(())
            }
        }
    }

    fn palette_candidates(&self) -> Vec<String> {
        let mut candidates: Vec<String> = Command::palette()
            .iter()
            .map(|command| command.name().to_string())
            .collect();
        for name in self.plugin_command_names() {
            if !candidates.contains(&name) {
                candidates.push(name);
            }
        }
        candidates
    }

    fn submit_file_prompt(&mut self, prompt: PromptState) -> Result<()> {
        match prompt.kind {
            PromptKind::OpenPath
            | PromptKind::QuickOpen
            | PromptKind::SaveAs
            | PromptKind::NewFile
            | PromptKind::NewDir
            | PromptKind::WorkspaceRoot
            | PromptKind::WorkspaceTerminalDir
            | PromptKind::Rename => self.submit_path_prompt(prompt),
            _ => self.submit_dialog_prompt(prompt),
        }
    }

    fn submit_path_prompt(&mut self, prompt: PromptState) -> Result<()> {
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
            PromptKind::NewFile => {
                let path = resolve(&self.state.explorer.target_dir(), &prompt.fields[0])?;
                self.dispatch(Command::CreateFile(path))
            }
            PromptKind::NewDir => {
                let path = resolve(&self.state.explorer.target_dir(), &prompt.fields[0])?;
                self.dispatch(Command::CreateDir(path))
            }
            PromptKind::WorkspaceRoot => self.submit_workspace_root(&prompt.fields[0]),
            PromptKind::WorkspaceTerminalDir => {
                let path = resolve(&self.state.explorer.root, &prompt.fields[0])?;
                self.set_workspace_terminal_dir(path)
            }
            PromptKind::Rename => {
                let name = prompt.fields[0].clone();
                self.dispatch(Command::RenameTo(PathBuf::from(name)))
            }
            _ => Ok(()),
        }
    }

    fn submit_dialog_prompt(&mut self, prompt: PromptState) -> Result<()> {
        match prompt.kind {
            PromptKind::Overwrite => match prompt.target.clone() {
                Some(path) => self.dispatch(Command::SaveTo(path)),
                None => Ok(()),
            },
            PromptKind::ExternalChange => self.submit_external_change(&prompt),
            PromptKind::Delete => match prompt.target.clone() {
                Some(path) => self.dispatch(Command::DeleteTarget(path)),
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn submit_git_prompt(&mut self, prompt: PromptState) -> Result<()> {
        match prompt.kind {
            PromptKind::CommitMessage => self.submit_commit(&prompt.fields[0]),
            PromptKind::GitStatus => {
                let selected = prompt.filtered.get(prompt.selected).copied();
                let Some(entry) = selected
                    .and_then(|index| self.git_status_entries.get(index))
                    .cloned()
                else {
                    return Ok(());
                };
                self.open_git_entry(entry)
            }
            PromptKind::Conflicts => {
                let selected = prompt.filtered.get(prompt.selected).copied();
                let Some(entry) = selected
                    .and_then(|index| self.git_status_entries.get(index))
                    .cloned()
                else {
                    return Ok(());
                };
                self.open_conflict_entry(entry)
            }
            _ => Ok(()),
        }
    }

    fn submit_workspace_root(&mut self, field: &str) -> Result<()> {
        let path = resolve(&self.state.explorer.root, field)?;
        self.set_workspace_root(path)
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
            PromptKind::RenameWorkspace => self.rename_workspace(&prompt.fields[0]),
            PromptKind::TerminalRename => self.rename_terminal(&prompt.fields[0]),
            PromptKind::QuitConfirm => {
                if prompt.selected_candidate() == Some("Quit") {
                    self.state.should_quit = true;
                }
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
            PromptKind::Recovery => {
                let choice = prompt
                    .selected_candidate()
                    .unwrap_or("Keep unsaved buffer")
                    .to_string();
                if let Some(id) = prompt.document {
                    self.resolve_recovery(id, &choice)?;
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
        let project_query = matches!(kind, PromptKind::FindInFiles | PromptKind::ReplaceInFiles)
            .then(|| prompt.fields.first().cloned().unwrap_or_default());
        self.state.mode = InputMode::Prompt(prompt);
        if let Some(query) = project_query {
            self.start_project_search(query);
        }
    }

    fn fill_workspace_candidates(&self, prompt: &mut PromptState) {
        prompt.set_candidates(
            self.state
                .workspaces
                .workspaces
                .iter()
                .enumerate()
                .map(|(index, workspace)| {
                    format!(
                        "{}  {}{}",
                        index + 1,
                        workspace.name,
                        if index == self.state.workspaces.active {
                            " *"
                        } else {
                            ""
                        }
                    )
                })
                .collect(),
        );
    }

    fn fill_search_prompt(&self, kind: PromptKind, prompt: &mut PromptState) {
        prompt.history.entries = self.search_history.list(kind).to_vec();
        let query = if matches!(kind, PromptKind::FindInFiles | PromptKind::ReplaceInFiles) {
            self.state
                .project_search
                .as_ref()
                .map(|search| search.query.clone())
        } else {
            self.state
                .search
                .as_ref()
                .map(|search| search.query.clone())
        };
        if let Some(query) = query {
            prompt.set_input(query);
        }
    }

    fn fill_query_prompt(&mut self, kind: PromptKind, prompt: &mut PromptState) -> bool {
        if kind == PromptKind::GitStatus {
            return self.fill_git_status(prompt);
        }
        if kind == PromptKind::Conflicts {
            return self.fill_conflicts(prompt);
        }
        match kind {
            PromptKind::CommandPalette => {
                prompt.set_candidates(self.palette_candidates());
            }
            PromptKind::SelectWorkspace => self.fill_workspace_candidates(prompt),
            PromptKind::Messages => {
                if self.state.warnings.is_empty() {
                    self.notify("no messages");
                    return false;
                }
                prompt.set_candidates(self.state.warnings.clone());
            }
            PromptKind::Find
            | PromptKind::Replace
            | PromptKind::FindInFiles
            | PromptKind::ReplaceInFiles => self.fill_search_prompt(kind, prompt),
            PromptKind::GoToLine => {
                if let (Some(pane), Some(document)) =
                    (self.state.focused_pane(), self.state.active_document())
                {
                    prompt.set_input((document.line_of_char(pane.cursor) + 1).to_string());
                }
            }
            PromptKind::Symbols => {
                let Some(document) = self.state.active_document() else {
                    self.notify("no active document");
                    return false;
                };
                let language = crate::syntax::Language::from_path(document.path.as_deref());
                let symbols = crate::structure::symbols(&document.rope, language);
                if symbols.is_empty() {
                    self.notify("no symbols in this document");
                    return false;
                }
                prompt.set_candidates(
                    symbols
                        .iter()
                        .map(|symbol| {
                            format!("{:>5}  {:<8} {}", symbol.line + 1, symbol.kind, symbol.name)
                        })
                        .collect(),
                );
                prompt.symbols = symbols;
            }
            _ => {}
        }
        true
    }

    fn fill_git_status(&mut self, prompt: &mut PromptState) -> bool {
        let binary = self.state.config.git.binary.clone();
        let Some(repo) = crate::git::discover(&binary, &self.state.explorer.root) else {
            self.notify("not a git repository");
            return false;
        };
        match crate::git::status(&binary, &repo) {
            Ok(entries) if entries.is_empty() => {
                self.notify("working tree clean");
                false
            }
            Ok(entries) => {
                prompt.set_candidates(
                    entries
                        .iter()
                        .map(|entry| format!("{}  {}", entry.code, entry.relative))
                        .collect(),
                );
                self.git_status_entries = entries;
                true
            }
            Err(error) => {
                self.notify(error);
                false
            }
        }
    }

    fn fill_conflicts(&mut self, prompt: &mut PromptState) -> bool {
        let binary = self.state.config.git.binary.clone();
        let Some(repo) = crate::git::discover(&binary, &self.state.explorer.root) else {
            self.notify("not a git repository");
            return false;
        };
        match crate::git::status(&binary, &repo) {
            Ok(entries) => {
                let conflicts: Vec<_> = entries
                    .into_iter()
                    .filter(crate::git::StatusEntry::is_conflict)
                    .collect();
                if conflicts.is_empty() {
                    self.notify("no merge conflicts");
                    return false;
                }
                prompt.set_candidates(
                    conflicts
                        .iter()
                        .map(|entry| format!("{}  {}", entry.code, entry.relative))
                        .collect(),
                );
                self.git_status_entries = conflicts;
                true
            }
            Err(error) => {
                self.notify(error);
                false
            }
        }
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
            PromptKind::WorkspaceRoot => {
                prompt.set_input(self.state.explorer.root.to_string_lossy().into_owned())
            }
            PromptKind::WorkspaceTerminalDir => prompt.set_input(
                self.state
                    .workspaces
                    .active()
                    .terminal_dir
                    .to_string_lossy()
                    .into_owned(),
            ),
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
        match kind {
            PromptKind::Theme => {
                prompt.set_candidates(crate::theme::Theme::names(&self.state.themes_dir));
                prompt.select_candidate(&self.state.config.appearance.theme);
            }
            PromptKind::RenameWorkspace => {
                prompt.set_input(self.state.workspaces.active().name.clone());
            }
            PromptKind::TerminalRename => {
                let Some(shell) = self.state.terminals.active() else {
                    self.notify("no terminal open");
                    return false;
                };
                prompt.set_input(shell.title().to_string());
            }
            _ => {}
        }
        true
    }

    pub(crate) fn quick_open_candidates(&mut self) -> Vec<String> {
        // Without a watcher the list cannot be invalidated, so keep rescanning.
        if self.watchers.is_empty() {
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
