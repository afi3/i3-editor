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
        question: String,
        attachments: Vec<AiAttachment>,
        used: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let query = question.clone();
        let _ = std::thread::Builder::new()
            .name("i3-semantic-query".to_string())
            .spawn(move || {
                let _ = tx.send(semantic_hits(&config, &model, &query, used));
            });
        Self {
            question,
            attachments,
            rx,
        }
    }

    pub(crate) fn try_recv(&self) -> Option<Hit> {
        self.rx.try_recv().ok()
    }
}

impl App {
    /// Rebuild the semantic index from open documents. Explicit on purpose: every
    /// run costs embedding calls, so nothing here happens on a timer.
    pub(crate) fn ai_semantic_index(&mut self) -> crate::error::Result<()> {
        let model = self.state.config.ai.embedding_model.trim().to_string();
        if model.is_empty() {
            self.notify("set ai.embedding_model before indexing");
            return Ok(());
        }
        let Some(path) = crate::semantic::default_path() else {
            self.notify("no config directory for the semantic index");
            return Ok(());
        };
        let max_chars = self.state.config.ai.semantic_max_chars.max(500);
        let previous = crate::semantic::Index::load(&path).filter(|index| index.model == model);
        let mut entries = Vec::new();
        let mut hashes = std::collections::BTreeMap::new();
        let mut files = 0usize;
        let mut reused = 0usize;
        let mut embedded = 0usize;
        for document in self.state.documents.values() {
            if document.read_only {
                continue;
            }
            let Some(file) = &document.path else {
                continue;
            };
            let text = document.rope().to_string();
            let hash = hash_text(&text);
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
                files += 1;
                reused += 1;
                continue;
            }
            let chunks: Vec<crate::semantic::Chunk> = crate::semantic::chunk(file, &text, 40)
                .into_iter()
                .filter(|chunk| chunk.text.chars().count() <= max_chars)
                .collect();
            if chunks.is_empty() {
                continue;
            }
            for batch in chunks.chunks(16) {
                let inputs: Vec<String> = batch.iter().map(|chunk| chunk.text.clone()).collect();
                let vectors = match crate::ai::embed(&self.state.config.ai, &model, &inputs) {
                    Ok(vectors) => vectors,
                    Err(error) => {
                        self.notify(format!("semantic index failed: {error}"));
                        return Ok(());
                    }
                };
                embedded += batch.len();
                for (chunk, vector) in batch.iter().zip(vectors) {
                    entries.push(crate::semantic::Entry {
                        chunk: chunk.clone(),
                        vector,
                    });
                }
            }
            files += 1;
        }
        if entries.is_empty() {
            self.notify("open a file to index");
            return Ok(());
        }

        let mut index = crate::semantic::Index::empty(model.clone());
        index.entries = entries;
        index.files = hashes;
        match index.save(&path) {
            Ok(()) => self.notify(format!(
                "semantic index: {} chunks from {files} file(s), {embedded} embedded, {reused} reused",
                index.entries.len()
            )),
            Err(error) => self.notify(format!("semantic index not saved: {error}")),
        }
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

fn semantic_hits(config: &AiConfig, model: &str, query: &str, used: usize) -> Hit {
    let mut notes = Vec::new();
    let Some(path) = crate::semantic::default_path() else {
        return Hit {
            attachments: Vec::new(),
            notes,
        };
    };
    let Some(index) = crate::semantic::Index::load(&path) else {
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
