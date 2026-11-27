//! Semantic-memory commands: build the embedding index and feed it to prompts.
//!
//! Query embedding runs on its own thread: a provider round-trip must never
//! freeze the editor while a prompt is submitted.

use std::sync::mpsc::{self, Receiver};

use crate::config::AiConfig;
use crate::state::AiAttachment;

use super::App;

/// Semantic hits for one question, plus the notes worth showing in the panel.
pub(crate) struct Hit {
    pub(crate) attachments: Vec<AiAttachment>,
    pub(crate) notes: Vec<String>,
}

/// One in-flight query embedding, holding the question it belongs to.
pub(crate) struct QueryTask {
    pub(crate) question: String,
    pub(crate) attachments: Vec<AiAttachment>,
    rx: Receiver<Hit>,
}

impl QueryTask {
    /// Start the embedding thread. Lexical attachments are carried along so the
    /// finished prompt keeps whatever the UI thread already gathered.
    pub(crate) fn spawn(
        config: AiConfig,
        model: String,
        index_path: std::path::PathBuf,
        question: String,
        attachments: Vec<AiAttachment>,
        used: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let query = question.clone();
        let _ = std::thread::Builder::new()
            .name("i3-semantic-query".to_string())
            .spawn(move || {
                let _ = tx.send(semantic_hits(&config, &model, &index_path, &query, used));
            });
        Self {
            question,
            attachments,
            rx,
        }
    }

    pub(crate) fn try_recv(&self) -> Option<Hit> {
        match self.rx.try_recv() {
            Ok(hit) => Some(hit),
            Err(mpsc::TryRecvError::Empty) => None,
            // A failed spawn or a panic inside the worker drops the sender; the
            // task must still finish, otherwise it would block every submit.
            Err(mpsc::TryRecvError::Disconnected) => Some(Hit {
                attachments: Vec::new(),
                notes: vec!["[context] semantic query worker stopped".to_string()],
            }),
        }
    }
}

/// One in-flight index build. The worker reads the files itself, so the UI
/// thread never copies buffer text.
pub(crate) struct IndexTask {
    rx: Receiver<Result<String, String>>,
}

impl IndexTask {
    pub(crate) fn spawn(
        config: AiConfig,
        model: String,
        index_path: std::path::PathBuf,
        paths: Vec<std::path::PathBuf>,
        max_chars: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("i3-semantic-index".to_string())
            .spawn(move || {
                let _ = tx.send(build_index(&config, &model, &index_path, &paths, max_chars));
            });
        Self { rx }
    }

    pub(crate) fn try_recv(&self) -> Option<Result<String, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("semantic index worker stopped".to_string()))
            }
        }
    }
}

fn build_index(
    config: &AiConfig,
    model: &str,
    path: &std::path::Path,
    paths: &[std::path::PathBuf],
    max_chars: usize,
) -> Result<String, String> {
    const MAX_FILE: u64 = 16 << 20;
    let mut unreadable = 0usize;
    let mut files: Vec<(std::path::PathBuf, String)> = Vec::new();
    for path in paths {
        let too_large = std::fs::metadata(path)
            .map(|meta| meta.len() > MAX_FILE)
            .unwrap_or(true);
        if too_large {
            unreadable += 1;
            continue;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => files.push((path.clone(), text)),
            Err(_) => unreadable += 1,
        }
    }
    if files.is_empty() {
        return Err("no readable file to index".to_string());
    }
    let previous = crate::semantic::Index::load(path).filter(|index| index.model == model);
    let mut entries = Vec::new();
    let mut hashes = std::collections::BTreeMap::new();
    let mut reused = 0usize;
    let mut embedded = 0usize;
    let mut indexed = 0usize;
    for (file, text) in &files {
        let hash = hash_text(text);
        hashes.insert(file.clone(), hash.clone());
        if let Some(previous) = &previous
            && previous.files.get(file) == Some(&hash)
        {
            entries.extend(
                previous
                    .entries
                    .iter()
                    .filter(|entry| &entry.chunk.path == file)
                    .cloned(),
            );
            reused += 1;
            continue;
        }
        let chunks: Vec<crate::semantic::Chunk> = crate::semantic::chunk(file, text, 40)
            .into_iter()
            .filter(|chunk| chunk.text.chars().count() <= max_chars)
            .collect();
        if chunks.is_empty() {
            continue;
        }
        indexed += 1;
        for batch in chunks.chunks(16) {
            let inputs: Vec<String> = batch.iter().map(|chunk| chunk.text.clone()).collect();
            let vectors = crate::ai::embed(config, model, &inputs)?;
            embedded += batch.len();
            for (chunk, vector) in batch.iter().zip(vectors) {
                entries.push(crate::semantic::Entry {
                    chunk: chunk.clone(),
                    vector,
                });
            }
        }
    }
    if entries.is_empty() {
        return Err("open a file to index".to_string());
    }
    let mut index = crate::semantic::Index::empty(model);
    index.entries = entries;
    index.files = hashes;
    index
        .save(path)
        .map_err(|error| format!("semantic index not saved: {error}"))?;
    let mut summary = format!(
        "semantic index: {} chunks from {} file(s), {embedded} embedded, {reused} reused",
        index.entries.len(),
        indexed
    );
    if unreadable > 0 {
        summary.push_str(&format!(", {unreadable} skipped"));
    }
    Ok(summary)
}

impl App {
    /// Rebuild the semantic index in the background. Explicit on purpose: every
    /// run costs embedding calls, so nothing here happens on a timer.
    pub(crate) fn ai_semantic_index(&mut self) -> crate::error::Result<()> {
        if self.semantic_index_task.is_some() {
            self.notify("semantic index build already running");
            return Ok(());
        }
        let model = self.state.config.ai.embedding_model.trim().to_string();
        if model.is_empty() {
            self.notify("set ai.embedding_model before indexing");
            return Ok(());
        }
        let mut skipped_dirty = 0usize;
        let mut paths = Vec::new();
        for document in self.state.documents.values() {
            if document.read_only {
                continue;
            }
            let Some(path) = &document.path else {
                continue;
            };
            if document.dirty {
                skipped_dirty += 1;
                continue;
            }
            paths.push(path.clone());
        }
        if paths.is_empty() {
            self.notify("open a saved file to index");
            return Ok(());
        }
        let max_chars = self.state.config.ai.semantic_max_chars.max(500);
        self.state.ai.entries.push(format!(
            "[context] building the semantic index with {model}"
        ));
        if skipped_dirty > 0 {
            self.state.ai.entries.push(format!(
                "[context] {skipped_dirty} dirty buffer(s) skipped; save them to index"
            ));
        }
        let Some(index_path) = crate::semantic::default_path() else {
            self.notify("no config directory for the semantic index");
            return Ok(());
        };
        self.semantic_index_task = Some(IndexTask::spawn(
            self.state.config.ai.clone(),
            model.clone(),
            index_path,
            paths,
            max_chars,
        ));
        self.notify(format!(
            "semantic index: building in the background with {model}"
        ));
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn ai_semantic_status(&mut self) {
        let Some(path) = crate::semantic::default_path() else {
            self.notify("no config directory for the semantic index");
            return;
        };
        match crate::semantic::Index::load(&path) {
            Some(index) => self.notify(format!(
                "semantic index: {} chunks, model {}, {}",
                index.entries.len(),
                index.model,
                path.display()
            )),
            None => self.notify(format!(
                "no semantic index at {}; run ai_semantic_index",
                path.display()
            )),
        }
    }

    /// Finish the question once its query embedding lands. Does nothing while the
    /// task is still running, so the loop keeps drawing and taking input.
    pub(crate) fn pump_semantic(&mut self) {
        if let Some(task) = self.semantic_index_task.as_ref()
            && let Some(result) = task.try_recv()
        {
            self.semantic_index_task = None;
            let message = match result {
                Ok(summary) => summary,
                Err(error) => format!("semantic index failed: {error}"),
            };
            self.state.ai.entries.push(format!("[context] {message}"));
            self.notify(message);
            self.dirty = true;
        }
        let Some(task) = self.semantic_task.as_ref() else {
            return;
        };
        let Some(hit) = task.try_recv() else {
            return;
        };
        let task = self.semantic_task.take().expect("checked above");
        for note in hit.notes {
            self.state.ai.entries.push(note);
        }
        let question = task.question;
        let mut attachments = task.attachments;
        attachments.extend(hit.attachments);
        self.finish_ai_question(question, attachments);
        self.dirty = true;
    }
}

fn semantic_hits(
    config: &AiConfig,
    model: &str,
    path: &std::path::Path,
    query: &str,
    used: usize,
) -> Hit {
    let mut notes = Vec::new();
    let Some(index) = crate::semantic::Index::load(path) else {
        notes.push("[context] semantic index missing".to_string());
        return Hit {
            attachments: Vec::new(),
            notes,
        };
    };
    if model.is_empty() || model != index.model {
        notes.push(format!(
            "[context] semantic index uses model {}; rebuild it",
            index.model
        ));
        return Hit {
            attachments: Vec::new(),
            notes,
        };
    }
    let Ok(mut vectors) = crate::ai::embed(config, model, &[query.to_string()]) else {
        notes.push("[context] semantic query embedding failed".to_string());
        return Hit {
            attachments: Vec::new(),
            notes,
        };
    };
    let Some(query_vector) = vectors.pop() else {
        return Hit {
            attachments: Vec::new(),
            notes,
        };
    };
    let budget = config.semantic_max_chars.max(500);
    let mut used = used;
    let mut attachments = Vec::new();
    for entry in crate::semantic::top_k(&index, &query_vector, config.semantic_top_k, 0.2) {
        let remaining = budget.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let text = crate::ai::truncate_text(&entry.chunk.text, remaining);
        let chars = text.chars().count();
        if chars == 0 {
            continue;
        }
        used += chars;
        let label = format!(
            "{}:{}-{} (semantic)",
            entry.chunk.path.display(),
            entry.chunk.start_line + 1,
            entry.chunk.end_line
        );
        notes.push(format!("[context] semantic attached {label}"));
        attachments.push(AiAttachment { path: label, text });
    }
    Hit { attachments, notes }
}

fn hash_text(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_query_worker_reports_instead_of_hanging() {
        let (tx, rx) = mpsc::channel::<Hit>();
        drop(tx);
        let task = QueryTask {
            question: "q".to_string(),
            attachments: Vec::new(),
            rx,
        };
        let hit = task.try_recv().expect("must not hang");
        assert!(
            hit.notes.iter().any(|note| note.contains("worker stopped")),
            "{:?}",
            hit.notes
        );
    }

    #[test]
    fn a_stopped_index_worker_reports_an_error() {
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        drop(tx);
        let task = IndexTask { rx };
        assert!(matches!(task.try_recv(), Some(Err(_))));
    }
}
