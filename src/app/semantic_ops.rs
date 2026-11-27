//! Semantic-memory commands: build the embedding index and feed it to prompts.

use super::App;

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

    /// Chunks closest to `query`, appended to the auto context while it fits.
    pub(crate) fn semantic_ai_context(
        &mut self,
        query: &str,
        used: &mut usize,
    ) -> Vec<crate::state::AiAttachment> {
        let Some(path) = crate::semantic::default_path() else {
            return Vec::new();
        };
        let Some(index) = crate::semantic::Index::load(&path) else {
            self.state
                .ai
                .entries
                .push("[context] semantic index missing".to_string());
            return Vec::new();
        };
        let model = self.state.config.ai.embedding_model.trim().to_string();
        if model.is_empty() || model != index.model {
            self.state.ai.entries.push(format!(
                "[context] semantic index uses model {}; rebuild it",
                index.model
            ));
            return Vec::new();
        }
        let Ok(mut vectors) = crate::ai::embed(&self.state.config.ai, &model, &[query.to_string()])
        else {
            return Vec::new();
        };
        let Some(query_vector) = vectors.pop() else {
            return Vec::new();
        };
        let budget = self.state.config.ai.semantic_max_chars.max(500);
        let mut attached = Vec::new();
        for entry in crate::semantic::top_k(
            &index,
            &query_vector,
            self.state.config.ai.semantic_top_k,
            0.2,
        ) {
            let remaining = budget.saturating_sub(*used);
            if remaining == 0 {
                break;
            }
            let text = crate::ai::truncate_text(&entry.chunk.text, remaining);
            let chars = text.chars().count();
            if chars == 0 {
                continue;
            }
            *used += chars;
            let label = format!(
                "{}:{}-{} (semantic)",
                entry.chunk.path.display(),
                entry.chunk.start_line + 1,
                entry.chunk.end_line
            );
            self.state
                .ai
                .entries
                .push(format!("[context] semantic attached {label}"));
            attached.push(crate::state::AiAttachment { path: label, text });
        }
        attached
    }
}

fn hash_text(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
