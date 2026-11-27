//! Minimal LSP client: one stdio server per configured language, full or
//! incremental document sync, diagnostics, completion, hover, and
//! go-to-definition.
//!
//! There is no async runtime. A reader thread per server turns framed JSON-RPC
//! messages into a channel; `LspManager::pump` drains that channel once per
//! frame, routes responses by request id, and keeps every write on the UI
//! thread. A server that fails to start is reported once and never retried.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::LspConfig;
use crate::editor::document::Document;
use crate::state::DocumentId;

/// Largest JSON-RPC body accepted from a server.
const MAX_MESSAGE_BYTES: usize = 8 << 20;

/// Interactive requests that never answer are dropped after this long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Startup may be slower (indexing); keep the server alive longer.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// Messages handled per server per pump; a server that floods diagnostics
/// cannot hold the frame open.
const DRAIN_BUDGET: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    /// Char column (converted from the protocol's UTF-16 units).
    pub column: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// Text inserted on Enter; `textEdit.newText`, `insertText`, or the label.
    pub insert: String,
    pub detail: String,
    /// Range the server asked to replace, in UTF-16 units. `None` means insert
    /// at the cursor, which is what `insertText` requests.
    pub range: Option<CompletionRange>,
}

/// A `textEdit` range; columns are UTF-16 code units, as the protocol defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionRange {
    pub start_line: usize,
    pub start_character: usize,
    pub end_line: usize,
    pub end_character: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: usize,
    /// UTF-16 code units; the app converts against the target line.
    pub character: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Diagnostics {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
    },
    Completion {
        /// Document and text generation the request was made against; a reply
        /// for an older revision is stale and must be dropped.
        document: DocumentId,
        revision: u64,
        items: Vec<CompletionItem>,
    },
    Hover {
        /// Document and text generation the request was made against.
        document: DocumentId,
        revision: u64,
        /// Empty when the server has nothing to say, which closes the popup.
        text: String,
    },
    Definition(Option<Location>),
    Warning(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplyKind {
    Initialize,
    Completion,
    Hover,
    Definition,
}

fn timeout_for(kind: ReplyKind) -> Duration {
    match kind {
        ReplyKind::Initialize => INITIALIZE_TIMEOUT,
        _ => REQUEST_TIMEOUT,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncKind {
    None,
    Full,
    Incremental,
}

struct QueuedRequest {
    key: String,
    method: &'static str,
    params: Value,
    kind: ReplyKind,
    target: Target,
}

/// Document and text generation a request was made against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    document: DocumentId,
    revision: u64,
}

struct PendingRequest {
    method: &'static str,
    kind: ReplyKind,
    /// Absent for handshake requests such as `initialize`.
    target: Option<Target>,
    sent: Instant,
}

struct OpenDocument {
    key: String,
    version: i64,
    revision: u64,
    text: String,
}

struct Server {
    writer: Box<dyn Write + Send>,
    incoming: Receiver<Value>,
    child: Option<Child>,
    next_id: i64,
    initialized: bool,
    /// The pipe broke: a write failed or the reader saw EOF.
    dead: bool,
    sync: SyncKind,
    pending: HashMap<i64, PendingRequest>,
}

impl Server {
    fn spawn(
        key: &str,
        config: &crate::config::LspServerConfig,
        root: &Path,
    ) -> std::io::Result<Self> {
        let mut child = Command::new(&config.command)
            .args(&config.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other(format!("{key}: server has no stdin")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other(format!("{key}: server has no stdout")))?;
        Ok(Self::from_parts(
            Box::new(stdin),
            Box::new(stdout),
            Some(child),
        ))
    }

    fn from_parts(
        writer: Box<dyn Write + Send>,
        reader: Box<dyn Read + Send>,
        child: Option<Child>,
    ) -> Self {
        let (sender, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            while let Some(message) = read_frame(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            writer,
            incoming,
            child,
            next_id: 1,
            initialized: false,
            dead: false,
            sync: SyncKind::Full,
            pending: HashMap::new(),
        }
    }

    fn request(
        &mut self,
        method: &'static str,
        params: Value,
        kind: ReplyKind,
        target: Option<Target>,
    ) -> Option<i64> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if write_frame(&mut self.writer, &frame).is_err() {
            self.dead = true;
            return None;
        }
        self.pending.insert(
            id,
            PendingRequest {
                method,
                kind,
                target,
                sent: Instant::now(),
            },
        );
        Some(id)
    }

    fn notify(&mut self, method: &str, params: Value) {
        let frame = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if write_frame(&mut self.writer, &frame).is_err() {
            self.dead = true;
        }
    }

    fn did_open(&mut self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri(path),
                "languageId": language_id(path),
                "version": 1,
                "text": text,
            }}),
        );
    }

    fn did_change(&mut self, path: &Path, version: i64, old: &str, new: &str) {
        if self.sync == SyncKind::None || old == new {
            return;
        }
        let changes = match self.sync {
            SyncKind::Incremental => {
                let (start, end, text) = incremental_change(old, new);
                json!([{
                    "range": {
                        "start": {"line": start.0, "character": start.1},
                        "end": {"line": end.0, "character": end.1},
                    },
                    "text": text,
                }])
            }
            SyncKind::Full => json!([{"text": new}]),
            SyncKind::None => return,
        };
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri(path), "version": version},
                "contentChanges": changes,
            }),
        );
    }

    fn did_close(&mut self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri(path)}}),
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Language servers for the open documents. Empty configuration means every
/// command reports that no server is configured.
pub struct LspManager {
    root: PathBuf,
    configs: Vec<(String, crate::config::LspServerConfig)>,
    servers: HashMap<String, Server>,
    failed: HashSet<String>,
    opened: HashMap<PathBuf, OpenDocument>,
    queued: Vec<QueuedRequest>,
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// Canonical paths by document path; `pump` runs on every frame and must
    /// not hit the filesystem for a path that has not changed.
    canonical_paths: HashMap<PathBuf, PathBuf>,
    /// Buffers copied for a server; test-only so the guard costs nothing.
    #[cfg(test)]
    materialized: usize,
}

impl LspManager {
    pub fn new(config: &LspConfig, root: PathBuf) -> Self {
        let configs = config
            .servers
            .iter()
            .filter(|(_, server)| !server.command.trim().is_empty())
            .map(|(key, server)| (key.clone(), server.clone()))
            .collect();
        Self {
            root,
            configs,
            servers: HashMap::new(),
            failed: HashSet::new(),
            opened: HashMap::new(),
            queued: Vec::new(),
            diagnostics: HashMap::new(),
            canonical_paths: HashMap::new(),
            #[cfg(test)]
            materialized: 0,
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.configs.is_empty()
    }

    pub fn supports(&self, path: &Path) -> bool {
        self.config_key(path).is_some()
    }

    /// Canonical path for `path`, cached; `pump` resolves paths every frame.
    fn canonical(&mut self, path: &Path) -> PathBuf {
        if let Some(cached) = self.canonical_paths.get(path) {
            return cached.clone();
        }
        let resolved = canonical(path);
        self.canonical_paths
            .insert(path.to_path_buf(), resolved.clone());
        resolved
    }

    /// Drop every server and start over against a new root or configuration.
    /// Open documents are reopened by the next `pump`.
    pub fn restart(&mut self, config: &LspConfig, root: PathBuf) {
        *self = Self::new(config, root);
    }

    /// Sync open documents and dispatch every queued server message. A
    /// document's text is only materialized when its revision changed.
    pub fn pump(&mut self, documents: &HashMap<DocumentId, Document>) -> Vec<Event> {
        let mut events = Vec::new();
        #[cfg(test)]
        let mut materialized = 0usize;
        let mut desired: Vec<(PathBuf, String, DocumentId)> = Vec::new();
        for (id, document) in documents {
            if document.read_only {
                continue;
            }
            let Some(path) = &document.path else {
                continue;
            };
            let Some(key) = self.config_key(path) else {
                continue;
            };
            desired.push((self.canonical(path), key, *id));
        }

        for (path, key, id) in &desired {
            self.ensure_server(key, &mut events);
            let Some(server) = self.servers.get_mut(key) else {
                continue;
            };
            if !server.initialized {
                continue;
            }
            let Some(document) = documents.get(id) else {
                continue;
            };
            if !self.opened.contains_key(path) {
                let text = document.rope().to_string();
                #[cfg(test)]
                {
                    materialized += 1;
                }
                server.did_open(path, &text);
                self.opened.insert(
                    path.clone(),
                    OpenDocument {
                        key: key.clone(),
                        version: 1,
                        revision: document.revision,
                        text,
                    },
                );
            } else if let Some(open) = self.opened.get_mut(path)
                && open.key == *key
                && open.revision != document.revision
            {
                let text = document.rope().to_string();
                #[cfg(test)]
                {
                    materialized += 1;
                }
                if server.sync != SyncKind::None {
                    open.version += 1;
                    server.did_change(path, open.version, &open.text, &text);
                }
                open.text = text;
                open.revision = document.revision;
            }
        }

        let desired_paths: HashSet<PathBuf> = desired.iter().map(|(p, _, _)| p.clone()).collect();
        let closed: Vec<PathBuf> = self
            .opened
            .keys()
            .filter(|path| !desired_paths.contains(*path))
            .cloned()
            .collect();
        for path in closed {
            if let Some(open) = self.opened.remove(&path)
                && let Some(server) = self.servers.get_mut(&open.key)
            {
                server.did_close(&path);
            }
        }

        let keys: Vec<String> = self.servers.keys().cloned().collect();
        for key in keys {
            self.drain(&key, &mut events);
        }
        self.expire(&mut events);
        #[cfg(test)]
        {
            self.materialized += materialized;
        }
        events
    }

    /// How many times a buffer was copied for a server; guards the revision
    /// check that keeps an idle document out of `to_string`.
    #[cfg(test)]
    pub(crate) fn materializations_for_test(&self) -> usize {
        self.materialized
    }

    fn expire(&mut self, events: &mut Vec<Event>) {
        self.expire_at(Instant::now(), events);
    }

    fn expire_at(&mut self, now: Instant, events: &mut Vec<Event>) {
        let mut expired: Vec<(String, &'static str, ReplyKind)> = Vec::new();
        for (key, server) in &mut self.servers {
            server.pending.retain(|_, pending| {
                if now.saturating_duration_since(pending.sent) < timeout_for(pending.kind) {
                    return true;
                }
                expired.push((key.clone(), pending.method, pending.kind));
                false
            });
        }
        let mut failed = Vec::new();
        for (key, method, kind) in expired {
            if kind == ReplyKind::Initialize {
                failed.push(key.clone());
            }
            events.push(Event::Warning(format!("LSP {key}: {method} timed out")));
        }
        for key in failed {
            self.servers.remove(&key);
            self.failed.insert(key.clone());
            self.queued.retain(|request| request.key != key);
            self.opened.retain(|_, open| open.key != key);
        }
    }

    fn ensure_server(&mut self, key: &str, events: &mut Vec<Event>) {
        if self.servers.contains_key(key) || self.failed.contains(key) {
            return;
        }
        let Some(config) = self
            .configs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, config)| config.clone())
        else {
            return;
        };
        match Server::spawn(key, &config, &self.root) {
            Ok(mut server) => {
                let params = initialize_params(&self.root);
                if server
                    .request("initialize", params, ReplyKind::Initialize, None)
                    .is_none()
                {
                    self.failed.insert(key.to_string());
                    events.push(Event::Warning(format!("LSP {key}: cannot send initialize")));
                    return;
                }
                self.servers.insert(key.to_string(), server);
            }
            Err(error) => {
                self.failed.insert(key.to_string());
                events.push(Event::Warning(format!(
                    "LSP {key} failed to start: {error}"
                )));
            }
        }
    }

    fn drain(&mut self, key: &str, events: &mut Vec<Event>) {
        let mut published = Vec::new();
        let mut initialized = false;
        let mut dead = false;
        {
            let Some(server) = self.servers.get_mut(key) else {
                return;
            };
            for _ in 0..DRAIN_BUDGET {
                let message = match server.incoming.try_recv() {
                    Ok(message) => message,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                };
                let Some(id) = message.get("id").and_then(Value::as_i64) else {
                    if message.get("method").and_then(Value::as_str)
                        == Some("textDocument/publishDiagnostics")
                        && let Some((path, diagnostics)) = parse_diagnostics(message.get("params"))
                    {
                        published.push((path, diagnostics));
                    }
                    continue;
                };
                let Some(pending) = server.pending.remove(&id) else {
                    continue;
                };
                match pending.kind {
                    ReplyKind::Initialize => {
                        initialized = true;
                        server.initialized = true;
                        server.sync = sync_kind_from(message.get("result"));
                        server.notify("initialized", json!({}));
                    }
                    ReplyKind::Completion => {
                        let Some(target) = pending.target else {
                            continue;
                        };
                        let items = parse_completion(message.get("result"));
                        events.push(Event::Completion {
                            document: target.document,
                            revision: target.revision,
                            items,
                        });
                    }
                    ReplyKind::Hover => {
                        let Some(target) = pending.target else {
                            continue;
                        };
                        events.push(Event::Hover {
                            document: target.document,
                            revision: target.revision,
                            text: parse_hover(message.get("result")),
                        });
                    }
                    ReplyKind::Definition => {
                        events.push(Event::Definition(parse_definition(message.get("result"))));
                    }
                }
            }
            dead |= server.dead;
        }
        for (path, diagnostics) in published {
            self.diagnostics.insert(path.clone(), diagnostics.clone());
            events.push(Event::Diagnostics { path, diagnostics });
        }
        if dead {
            // The pipe closed or a write failed. Drop the server, its queued
            // requests, its open documents, and its diagnostics, and report it
            // once; `failed` keeps it from being retried this session.
            let orphaned: Vec<PathBuf> = self
                .opened
                .iter()
                .filter(|(_, open)| open.key == key)
                .map(|(path, _)| path.clone())
                .collect();
            self.opened.retain(|_, open| open.key != key);
            self.queued.retain(|request| request.key != key);
            for path in orphaned {
                self.diagnostics.remove(&path);
                events.push(Event::Diagnostics {
                    path,
                    diagnostics: Vec::new(),
                });
            }
            self.servers.remove(key);
            self.failed.insert(key.to_string());
            events.push(Event::Warning(format!(
                "LSP {key} exited; completion and diagnostics are off"
            )));
            return;
        }
        if initialized {
            let queued: Vec<QueuedRequest> = self
                .queued
                .iter()
                .filter(|request| request.key == key)
                .map(|request| QueuedRequest {
                    key: request.key.clone(),
                    method: request.method,
                    params: request.params.clone(),
                    kind: request.kind,
                    target: request.target,
                })
                .collect();
            self.queued.retain(|request| request.key != key);
            if let Some(server) = self.servers.get_mut(key) {
                for request in queued {
                    server.request(
                        request.method,
                        request.params,
                        request.kind,
                        Some(request.target),
                    );
                }
            }
        }
    }

    pub fn completion(&mut self, document: &Document, cursor: usize) -> Result<(), String> {
        self.request(
            document,
            cursor,
            "textDocument/completion",
            ReplyKind::Completion,
        )
    }

    pub fn hover(&mut self, document: &Document, cursor: usize) -> Result<(), String> {
        self.request(document, cursor, "textDocument/hover", ReplyKind::Hover)
    }

    pub fn definition(&mut self, document: &Document, cursor: usize) -> Result<(), String> {
        self.request(
            document,
            cursor,
            "textDocument/definition",
            ReplyKind::Definition,
        )
    }

    fn request(
        &mut self,
        document: &Document,
        cursor: usize,
        method: &'static str,
        kind: ReplyKind,
    ) -> Result<(), String> {
        let Some(path) = document.path.as_deref() else {
            return Err("language server: document has no path".to_string());
        };
        let Some(key) = self.config_key(path) else {
            return Err(format!(
                "no language server configured for {}",
                path.display()
            ));
        };
        let path = canonical(path);
        let params = position_params(document, &path, cursor);
        match self.servers.get_mut(&key) {
            Some(server) if server.initialized => {
                server
                    .request(
                        method,
                        params,
                        kind,
                        Some(Target {
                            document: document.id,
                            revision: document.revision,
                        }),
                    )
                    .ok_or_else(|| format!("LSP {key}: cannot send {method}"))?;
            }
            Some(_) => self.queued.push(QueuedRequest {
                key,
                method,
                params,
                kind,
                target: Target {
                    document: document.id,
                    revision: document.revision,
                },
            }),
            None => {
                let mut events = Vec::new();
                self.ensure_server(&key, &mut events);
                if self.failed.contains(&key) {
                    return Err(format!("LSP {key}: server unavailable"));
                }
                self.queued.push(QueuedRequest {
                    key,
                    method,
                    params,
                    kind,
                    target: Target {
                        document: document.id,
                        revision: document.revision,
                    },
                });
            }
        }
        Ok(())
    }

    fn config_key(&self, path: &Path) -> Option<String> {
        let filename = path.file_name().map(|name| name.to_string_lossy());
        for (key, config) in &self.configs {
            if filename.as_ref().is_some_and(|filename| {
                config
                    .filenames
                    .iter()
                    .any(|expected| crate::config::filename_matches(expected, filename))
            }) {
                return Some(key.clone());
            }
        }
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())?;
        for (key, config) in &self.configs {
            if !config.extensions.is_empty() {
                if config
                    .extensions
                    .iter()
                    .any(|ext| ext.trim_start_matches('.').eq_ignore_ascii_case(&extension))
                {
                    return Some(key.clone());
                }
                continue;
            }
            if default_extensions(key).contains(&extension.as_str()) {
                return Some(key.clone());
            }
        }
        None
    }
}

fn initialize_params(root: &Path) -> Value {
    json!({
        "processId": std::process::id(),
        "rootUri": uri(root),
        "rootPath": root.to_string_lossy(),
        "capabilities": {
            "textDocument": {
                "synchronization": {"dynamicRegistration": false},
                "completion": {"completionItem": {"snippetSupport": false}},
                "hover": {"contentFormat": ["plaintext", "markdown"]},
                "definition": {}
            }
        },
    })
}

/// Read the server's change kind from its `initialize` result. A missing
/// capability keeps the previous full-document behaviour.
fn sync_kind_from(result: Option<&Value>) -> SyncKind {
    let Some(sync) = result
        .and_then(|result| result.get("capabilities"))
        .and_then(|capabilities| capabilities.get("textDocumentSync"))
    else {
        return SyncKind::Full;
    };
    let change = match sync {
        Value::Number(number) => number.as_u64(),
        Value::Object(object) => object.get("change").and_then(Value::as_u64),
        _ => None,
    };
    match change {
        Some(0) => SyncKind::None,
        Some(2) => SyncKind::Incremental,
        _ => SyncKind::Full,
    }
}

/// One contiguous replacement that turns `old` into `new`, with positions in
/// the old document. UTF-16 columns match the protocol's position units.
fn incremental_change<'a>(old: &str, new: &'a str) -> ((usize, usize), (usize, usize), &'a str) {
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();
    let mut prefix = 0;
    let limit = old_bytes.len().min(new_bytes.len());
    while prefix < limit && old_bytes[prefix] == new_bytes[prefix] {
        prefix += 1;
    }
    while prefix > 0 && !old.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = 0;
    let limit = (old_bytes.len() - prefix).min(new_bytes.len() - prefix);
    while suffix < limit
        && old_bytes[old_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(old_bytes.len() - suffix)
            || !new.is_char_boundary(new_bytes.len() - suffix))
    {
        suffix -= 1;
    }
    let old_end = old_bytes.len() - suffix;
    let new_end = new_bytes.len() - suffix;
    (
        position_at(old, prefix),
        position_at(old, old_end),
        &new[prefix..new_end],
    )
}

/// Line and UTF-16 column at a byte offset.
fn position_at(text: &str, byte: usize) -> (usize, usize) {
    let mut line = 0;
    let mut character = 0;
    for ch in text[..byte].chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16();
        }
    }
    (line, character)
}

fn position_params(document: &Document, path: &Path, cursor: usize) -> Value {
    let line = document.line_of_char(cursor);
    let line_start = document.char_of_line(line);
    let text = document.line_text(line);
    let character = char_to_utf16(&text, cursor.saturating_sub(line_start));
    json!({
        "textDocument": {"uri": uri(path)},
        "position": {"line": line, "character": character},
    })
}

fn parse_diagnostics(params: Option<&Value>) -> Option<(PathBuf, Vec<Diagnostic>)> {
    let params = params?;
    let path = path_from_uri(params.get("uri")?.as_str()?)?;
    let items = params.get("diagnostics")?.as_array()?;
    let diagnostics = items
        .iter()
        .map(|item| {
            let range = item.get("range").and_then(|range| range.get("start"));
            let line = range
                .and_then(|start| start.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let character = range
                .and_then(|start| start.get("character"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let severity = match item.get("severity").and_then(Value::as_u64) {
                Some(1) => Severity::Error,
                Some(2) => Severity::Warning,
                _ => Severity::Info,
            };
            Diagnostic {
                line,
                column: character,
                severity,
                message: item
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect();
    Some((path, diagnostics))
}

fn parse_completion(result: Option<&Value>) -> Vec<CompletionItem> {
    let Some(result) = result else {
        return Vec::new();
    };
    let items = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array));
    let Some(items) = items else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            let label = item
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let text_edit = item.get("textEdit");
            let insert = text_edit
                .and_then(|edit| edit.get("newText"))
                .and_then(Value::as_str)
                .or_else(|| item.get("insertText").and_then(Value::as_str))
                .unwrap_or(&label)
                .to_string();
            // `InsertReplaceEdit` carries both ranges; plain accept inserts
            // over the `insert` range.
            let range = text_edit
                .and_then(|edit| edit.get("insert").or_else(|| edit.get("range")))
                .and_then(parse_range);
            let detail = item
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            CompletionItem {
                label,
                insert,
                detail,
                range,
            }
        })
        .collect()
}

fn parse_range(value: &Value) -> Option<CompletionRange> {
    let start = value.get("start")?;
    let end = value.get("end")?;
    Some(CompletionRange {
        start_line: start.get("line")?.as_u64()? as usize,
        start_character: start.get("character")?.as_u64()? as usize,
        end_line: end.get("line")?.as_u64()? as usize,
        end_character: end.get("character")?.as_u64()? as usize,
    })
}

fn parse_hover(result: Option<&Value>) -> String {
    fn content(value: &Value) -> Option<String> {
        if let Some(text) = value.as_str() {
            return Some(text.to_string());
        }
        if let Some(text) = value.get("value").and_then(Value::as_str) {
            return Some(text.to_string());
        }
        if let Some(items) = value.as_array() {
            let parts: Vec<String> = items.iter().filter_map(content).collect();
            if !parts.is_empty() {
                return Some(parts.join("\n"));
            }
        }
        None
    }
    result
        .and_then(|result| result.get("contents"))
        .and_then(content)
        .unwrap_or_default()
}

fn parse_definition(result: Option<&Value>) -> Option<Location> {
    let result = result?;
    let first = match result.as_array() {
        Some(items) => items.first()?,
        None => result,
    };
    let uri = first
        .get("uri")
        .or_else(|| first.get("targetUri"))
        .and_then(Value::as_str)?;
    let path = path_from_uri(uri)?;
    let start = first
        .get("range")
        .and_then(|range| range.get("start"))
        .or_else(|| {
            first
                .get("targetSelectionRange")
                .and_then(|range| range.get("start"))
        })?;
    Some(Location {
        path,
        line: start.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
        character: start.get("character").and_then(Value::as_u64).unwrap_or(0) as usize,
    })
}

fn default_extensions(key: &str) -> &'static [&'static str] {
    match key.to_ascii_lowercase().as_str() {
        "rust" | "rust-analyzer" => &["rs"],
        "python" | "pyright" | "pylsp" => &["py", "pyi"],
        "javascript" | "typescript" | "tsserver" => &["js", "jsx", "mjs", "cjs", "ts", "tsx"],
        "go" | "gopls" => &["go"],
        "c" | "clangd" | "cpp" => &["c", "h", "cc", "cpp", "hpp", "cxx"],
        "lua" | "lua-language-server" => &["lua"],
        "yaml" | "yaml-language-server" => &["yml", "yaml"],
        "shell" | "bash" | "bash-language-server" => &["sh", "bash", "zsh"],
        "toml" | "taplo" => &["toml"],
        "json" | "jsonls" => &["json"],
        "markdown" | "marksman" => &["md", "markdown"],
        _ => &[],
    }
}

fn is_dockerfile(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            [
                "Dockerfile",
                "Containerfile",
                "Dockerfile.*",
                "Containerfile.*",
                "*.Dockerfile",
                "*.Containerfile",
            ]
            .iter()
            .any(|pattern| crate::config::filename_matches(pattern, name))
        })
}

pub fn language_id(path: &Path) -> &'static str {
    if is_dockerfile(path) {
        return "dockerfile";
    }
    path.extension()
        .map(|extension| language_for_extension(&extension.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or("plaintext")
}

fn language_for_extension(extension: &str) -> &'static str {
    match extension {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" => "c",
        "sh" | "bash" | "zsh" => "shellscript",
        "lua" => "lua",
        "yml" | "yaml" => "yaml",
        "dockerfile" => "dockerfile",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "json" => "json",
        _ => "plaintext",
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn uri(path: &Path) -> String {
    let path = canonical(path);
    let mut out = String::from("file://");
    for &byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let bytes_in = rest.as_bytes();
    let mut index = 0;
    while index < bytes_in.len() {
        match bytes_in[index] {
            b'%' if index + 2 < bytes_in.len() => {
                let hex = std::str::from_utf8(&bytes_in[index + 1..index + 3]).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte => {
                bytes.push(byte);
                index += 1;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// Convert a UTF-16 column into a char column, clamped to `text`.
pub fn utf16_to_char(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (chars, ch) in text.chars().enumerate() {
        if units >= utf16 {
            return chars;
        }
        units += ch.len_utf16();
    }
    text.chars().count()
}

/// Convert a char column into a UTF-16 column.
pub fn char_to_utf16(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}

pub fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length?;
    if length > MAX_MESSAGE_BYTES {
        return None;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

pub fn write_frame(writer: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(all(test, unix))]
impl LspManager {
    /// Attach an in-process transport instead of spawning a process. The peer
    /// end stays with the caller for the fake server thread.
    pub(crate) fn attach_for_test(
        &mut self,
        key: &str,
        stream: std::os::unix::net::UnixStream,
    ) -> std::io::Result<()> {
        let reader = Box::new(stream.try_clone()?);
        let writer = Box::new(stream);
        let mut server = Server::from_parts(writer, reader, None);
        if server
            .request(
                "initialize",
                initialize_params(&self.root),
                ReplyKind::Initialize,
                None,
            )
            .is_none()
        {
            return Err(std::io::Error::other("initialize failed"));
        }
        self.servers.insert(key.to_string(), server);
        Ok(())
    }

    pub(crate) fn config_for_test(&self, key: &str) -> bool {
        self.configs.iter().any(|(name, _)| name == key)
    }

    pub(crate) fn root_for_test(&self) -> &Path {
        &self.root
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::LspConfig;
    use serde_json::json;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    fn manager() -> LspManager {
        let mut config = LspConfig::default();
        config.servers.insert(
            "fake".to_string(),
            crate::config::LspServerConfig {
                command: "fake".to_string(),
                args: Vec::new(),
                extensions: vec!["rs".to_string()],
                filenames: Vec::new(),
            },
        );
        LspManager::new(&config, std::env::temp_dir())
    }

    fn document(dir: &Path) -> (DocumentId, Document) {
        let path = dir.join("main.rs");
        std::fs::write(&path, "fn main() {}\n").expect("seed");
        let id = DocumentId(1);
        (id, Document::open(id, path).expect("open"))
    }

    fn fake_server(peer: UnixStream, script: impl Fn(&Value) -> Option<Value> + Send + 'static) {
        std::thread::spawn(move || {
            let read = peer.try_clone().expect("clone");
            let mut reader = BufReader::new(read);
            let mut writer = peer;
            while let Some(message) = read_frame(&mut reader) {
                if let Some(reply) = script(&message)
                    && write_frame(&mut writer, &reply).is_err()
                {
                    break;
                }
            }
        });
    }

    #[derive(Default)]
    struct FeatureReplies {
        completion: Option<Vec<CompletionItem>>,
        hover: Option<String>,
        definition: Option<Option<Location>>,
    }

    impl FeatureReplies {
        fn ready(&self) -> bool {
            self.completion.is_some() && self.hover.is_some() && self.definition.is_some()
        }
    }

    fn collect_features(
        manager: &mut LspManager,
        documents: &HashMap<DocumentId, Document>,
        replies: &mut FeatureReplies,
    ) {
        for event in manager.pump(documents) {
            match event {
                Event::Completion { items, .. } => replies.completion = Some(items),
                Event::Hover { text, .. } => replies.hover = Some(text),
                Event::Definition(location) => replies.definition = Some(location),
                _ => {}
            }
        }
    }

    fn pump_until(
        manager: &mut LspManager,
        documents: &HashMap<DocumentId, Document>,
        deadline: Duration,
    ) -> Vec<Event> {
        let end = Instant::now() + deadline;
        let mut events = Vec::new();
        while Instant::now() < end {
            events.extend(manager.pump(documents));
            if events
                .iter()
                .any(|event| matches!(event, Event::Diagnostics { .. }))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        events
    }

    #[test]
    fn initialize_opens_the_document_and_reports_diagnostics() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let mut manager = manager();
        let (stream, peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        assert!(manager.config_for_test("fake"));
        fake_server(peer, |message| {
            match message.get("method").and_then(Value::as_str) {
                Some("initialize") => Some(json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": {"capabilities": {}}
                })),
                Some("textDocument/didOpen") => Some(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": message["params"]["textDocument"]["uri"],
                        "diagnostics": [{
                            "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}},
                            "severity": 1,
                            "message": "unknown name"
                        }]
                    }
                })),
                _ => None,
            }
        });
        let mut documents = HashMap::new();
        documents.insert(id, document);
        let events = pump_until(&mut manager, &documents, Duration::from_secs(5));
        let diagnostics = events
            .iter()
            .find_map(|event| match event {
                Event::Diagnostics { diagnostics, .. } => Some(diagnostics.clone()),
                _ => None,
            })
            .expect("diagnostics");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 0);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].message, "unknown name");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incremental_diffs_are_contiguous_and_utf16() {
        let old = "alpha\nbeta\ngamma\n";
        let new = "alpha\nbeta!\ngamma\n";
        let (start, end, text) = incremental_change(old, new);
        assert_eq!((start, end, text), ((1, 4), (1, 4), "!"));

        let (start, end, text) = incremental_change("é\n", "ê\n");
        assert_eq!((start, end, text), ((0, 0), (0, 1), "ê"));
    }

    #[test]
    fn sync_kind_follows_the_server_capability() {
        assert_eq!(
            sync_kind_from(Some(&json!({"capabilities": {"textDocumentSync": 0}}))),
            SyncKind::None
        );
        assert_eq!(
            sync_kind_from(Some(&json!({"capabilities": {"textDocumentSync": 2}}))),
            SyncKind::Incremental
        );
        assert_eq!(
            sync_kind_from(Some(
                &json!({"capabilities": {"textDocumentSync": {"change": 1}}})
            )),
            SyncKind::Full
        );
        assert_eq!(
            sync_kind_from(Some(&json!({"capabilities": {}}))),
            SyncKind::Full
        );
    }

    #[test]
    fn incremental_sync_sends_only_the_edited_range() {
        use std::sync::{Arc, Mutex};

        let dir = std::env::temp_dir().join(format!("i3-lsp-incremental-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let mut manager = manager();
        let (stream, peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        fake_server(peer, move |message| {
            if message.get("method").and_then(Value::as_str) == Some("textDocument/didChange") {
                captured.lock().expect("lock").push(message.clone());
            }
            match message.get("method").and_then(Value::as_str) {
                Some("initialize") => Some(json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": {"capabilities": {"textDocumentSync": {"change": 2}}}
                })),
                _ => None,
            }
        });
        let mut documents = HashMap::new();
        documents.insert(id, document);

        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && manager.opened.is_empty() {
            manager.pump(&documents);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!manager.opened.is_empty(), "document never opened");

        documents
            .get_mut(&id)
            .expect("document")
            .insert(11, "!", 0, (0, 0));
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && seen.lock().expect("lock").is_empty() {
            manager.pump(&documents);
            std::thread::sleep(Duration::from_millis(5));
        }
        let seen = seen.lock().expect("lock");
        let first = seen.first().expect("didChange");
        let change = &first["params"]["contentChanges"][0];
        assert_eq!(change["text"], "!");
        assert_eq!(change["range"]["start"]["line"], 0);
        assert_eq!(change["range"]["start"]["character"], 11);
        assert_eq!(change["range"]["end"], change["range"]["start"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_hover_and_definition_replies_are_parsed() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-features-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let target = dir.join("other.rs");
        std::fs::write(&target, "fn other() {}\n").expect("target");
        let target_uri = uri(&target);
        let mut manager = manager();
        let (stream, peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        fake_server(peer, move |message| {
            let method = message.get("method").and_then(Value::as_str);
            let result = match method {
                Some("initialize") => json!({"capabilities": {}}),
                Some("textDocument/completion") => json!({
                    "items": [
                        {"label": "answer", "detail": "u32", "insertText": "answer"},
                        {"label": "builder", "textEdit": {
                            "newText": "builder",
                            "range": {
                                "start": {"line": 0, "character": 0},
                                "end": {"line": 0, "character": 3}
                            }
                        }}
                    ]
                }),
                Some("textDocument/hover") => {
                    json!({"contents": {"kind": "plaintext", "value": "u32"}})
                }
                Some("textDocument/definition") => json!([{
                    "uri": target_uri,
                    "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 8}}
                }]),
                _ => return None,
            };
            Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
        });
        let mut documents = HashMap::new();
        documents.insert(id, document);

        // Let initialize and didOpen settle.
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !manager.servers["fake"].initialized {
            manager.pump(&documents);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(manager.servers["fake"].initialized);

        let document = &documents[&id];
        manager.completion(document, 3).expect("completion");
        manager.hover(document, 3).expect("hover");
        manager.definition(document, 3).expect("definition");

        let mut replies = FeatureReplies::default();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !replies.ready() {
            collect_features(&mut manager, &documents, &mut replies);
            std::thread::sleep(Duration::from_millis(5));
        }
        let completion = replies.completion.expect("completion");
        assert_eq!(completion[0].label, "answer");
        assert_eq!(completion[0].detail, "u32");
        assert_eq!(completion[0].range, None);
        assert_eq!(
            completion[1].range,
            Some(CompletionRange {
                start_line: 0,
                start_character: 0,
                end_line: 0,
                end_character: 3,
            })
        );
        assert_eq!(replies.hover.expect("hover"), "u32");
        let definition = replies.definition.expect("definition").expect("location");
        assert_eq!(
            definition.path,
            std::fs::canonicalize(&target).expect("canonical target")
        );
        assert_eq!(definition.line, 0);
        assert_eq!(definition.character, 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_hover_reply_still_reaches_the_app() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-hover-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let mut manager = manager();
        let (stream, peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        fake_server(peer, |message| {
            let result = match message.get("method").and_then(Value::as_str) {
                Some("initialize") => json!({"capabilities": {}}),
                Some("textDocument/hover") => json!({"contents": ""}),
                _ => return None,
            };
            Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
        });
        let mut documents = HashMap::new();
        documents.insert(id, document);

        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !manager.servers["fake"].initialized {
            manager.pump(&documents);
            std::thread::sleep(Duration::from_millis(5));
        }
        manager.hover(&documents[&id], 0).expect("hover");

        let mut hover = None;
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && hover.is_none() {
            for event in manager.pump(&documents) {
                if let Event::Hover { text, .. } = event {
                    hover = Some(text);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            hover.as_deref(),
            Some(""),
            "an empty reply must arrive so the popup closes"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unchanged_document_is_not_serialized_again() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-nocopy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let mut manager = manager();
        let (stream, peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        fake_server(peer, |message| {
            let result = match message.get("method").and_then(Value::as_str) {
                Some("initialize") => json!({"capabilities": {}}),
                _ => return None,
            };
            Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
        });
        let mut documents = HashMap::new();
        documents.insert(id, document);

        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end && !manager.servers["fake"].initialized {
            manager.pump(&documents);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(manager.servers["fake"].initialized);
        // The handshake lands in the same pump that still saw the server as
        // uninitialized, so one more pump performs `didOpen`.
        manager.pump(&documents);
        let opened = manager.materializations_for_test();
        assert_eq!(opened, 1, "didOpen needs the text exactly once");

        for _ in 0..5 {
            manager.pump(&documents);
        }
        assert_eq!(
            manager.materializations_for_test(),
            opened,
            "an idle document must not be copied again"
        );

        documents
            .get_mut(&id)
            .expect("document")
            .insert(11, "!", 0, (0, 0));
        manager.pump(&documents);
        assert_eq!(
            manager.materializations_for_test(),
            opened + 1,
            "one edit copies the buffer once"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_server_that_closes_its_pipe_is_reported_and_dropped() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-death-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let mut manager = manager();
        let (stream, peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        std::thread::spawn(move || {
            let read = peer.try_clone().expect("clone");
            let mut reader = BufReader::new(read);
            let mut writer = peer;
            // Answer the handshake, then close the pipe like a crashed server.
            if let Some(message) = read_frame(&mut reader) {
                let _ = write_frame(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {"capabilities": {}}
                    }),
                );
            }
        });
        let mut documents = HashMap::new();
        documents.insert(id, document);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut warning = None;
        while Instant::now() < deadline && warning.is_none() {
            for event in manager.pump(&documents) {
                if let Event::Warning(text) = event {
                    warning = Some(text);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let warning = warning.expect("server death reported");
        assert!(warning.contains("fake"), "{warning}");
        assert!(manager.servers.is_empty(), "dead server must be dropped");
        assert!(manager.failed.contains("fake"), "must not be retried");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restart_drops_servers_and_uses_new_root() {
        let mut manager = manager();
        let (stream, _peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        assert_eq!(manager.servers.len(), 1);

        let root = std::env::temp_dir().join(format!("i3-lsp-root-{}", std::process::id()));
        let config = LspConfig {
            servers: manager.configs.iter().cloned().collect(),
        };
        manager.restart(&config, root.clone());
        assert_eq!(manager.root, root);
        assert!(manager.servers.is_empty());
        assert!(manager.failed.is_empty());
        assert!(manager.opened.is_empty());
        assert!(manager.queued.is_empty());
    }

    #[test]
    fn unanswered_initialize_marks_the_server_failed_and_drops_queued_work() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-init-timeout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (_, document) = document(&dir);
        let mut manager = manager();
        let (stream, _peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        manager.completion(&document, 0).expect("queued completion");
        assert_eq!(manager.queued.len(), 1);

        let stale = Instant::now()
            .checked_sub(INITIALIZE_TIMEOUT + Duration::from_secs(1))
            .expect("instant");
        for pending in manager
            .servers
            .get_mut("fake")
            .expect("server")
            .pending
            .values_mut()
        {
            pending.sent = stale;
        }
        let mut events = Vec::new();
        manager.expire_at(Instant::now(), &mut events);

        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::Warning(message) if message.contains("initialize") && message.contains("timed out")
            )),
            "{events:?}"
        );
        assert!(manager.servers.is_empty());
        assert!(manager.failed.contains("fake"));
        assert!(manager.queued.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unanswered_feature_request_is_dropped() {
        let dir = std::env::temp_dir().join(format!("i3-lsp-req-timeout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (id, document) = document(&dir);
        let mut manager = manager();
        let (stream, _peer) = UnixStream::pair().expect("pair");
        manager.attach_for_test("fake", stream).expect("attach");
        manager.servers.get_mut("fake").expect("server").initialized = true;
        manager.completion(&document, 0).expect("completion");

        let stale = Instant::now()
            .checked_sub(REQUEST_TIMEOUT + Duration::from_secs(1))
            .expect("instant");
        for pending in manager
            .servers
            .get_mut("fake")
            .expect("server")
            .pending
            .values_mut()
        {
            pending.sent = stale;
        }
        let mut documents = HashMap::new();
        documents.insert(id, document);
        let events = manager.pump(&documents);

        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::Warning(message) if message.contains("completion") && message.contains("timed out")
            )),
            "{events:?}"
        );
        assert!(
            manager.servers["fake"]
                .pending
                .values()
                .all(|pending| pending.method != "textDocument/completion")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn utf16_columns_round_trip() {
        let text = "a😀b";
        assert_eq!(char_to_utf16(text, 2), 3);
        assert_eq!(utf16_to_char(text, 3), 2);
        assert_eq!(utf16_to_char(text, 99), 3);
    }

    #[test]
    fn uris_round_trip_percent_encoding() {
        let path = PathBuf::from("/tmp/a b/ü.txt");
        let encoded = uri(&path);
        assert!(encoded.contains("%20"), "{encoded}");
        assert_eq!(path_from_uri(&encoded).as_deref(), Some(path.as_path()));
    }

    #[test]
    fn extension_mapping_selects_a_server() {
        let manager = manager();
        assert_eq!(
            manager.config_key(Path::new("/tmp/main.rs")).as_deref(),
            Some("fake")
        );
        assert!(manager.config_key(Path::new("/tmp/main.py")).is_none());
    }

    #[test]
    fn dockerfile_filenames_and_yaml_ids_match() {
        let mut config = LspConfig::default();
        config.servers.insert(
            "dockerfile".to_string(),
            crate::config::LspServerConfig {
                command: "docker-langserver".to_string(),
                args: Vec::new(),
                extensions: Vec::new(),
                filenames: vec!["Dockerfile".to_string()],
            },
        );
        let manager = LspManager::new(&config, std::env::temp_dir());
        assert_eq!(
            manager.config_key(Path::new("/tmp/Dockerfile")).as_deref(),
            Some("dockerfile")
        );
        assert_eq!(language_id(Path::new("/tmp/Dockerfile")), "dockerfile");
        assert_eq!(language_id(Path::new("/tmp/Dockerfile.dev")), "dockerfile");
        assert_eq!(language_id(Path::new("/tmp/app.Dockerfile")), "dockerfile");
        assert_eq!(language_id(Path::new("/tmp/ci.yml")), "yaml");
    }

    #[test]
    fn no_configuration_reports_missing_server() {
        let config = LspConfig::default();
        let mut manager = LspManager::new(&config, std::env::temp_dir());
        let dir = std::env::temp_dir().join(format!("i3-lsp-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (_, document) = document(&dir);
        let error = manager.completion(&document, 0).expect_err("no server");
        assert!(error.contains("no language server"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
