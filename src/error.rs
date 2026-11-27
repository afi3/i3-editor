use std::path::PathBuf;

use crate::state::{DocumentId, PaneId};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unknown pane {0}")]
    UnknownPane(PaneId),
    #[error("unknown document {0}")]
    UnknownDocument(DocumentId),
    #[error("no focused pane")]
    NoFocusedPane,
    #[error("no active document")]
    NoActiveDocument,
    #[error("document has no path; use Save As")]
    NoPath,
    #[error("cannot split: not enough space for a minimum pane")]
    NoSpaceForSplit,
    #[error("cannot close the last pane")]
    LastPane,
    #[error("cannot close the last workspace")]
    LastWorkspace,
    #[error("unknown workspace {0}")]
    UnknownWorkspace(usize),
    #[error("{path} is not valid UTF-8")]
    InvalidUtf8 { path: PathBuf },
    #[error("{path} is too large ({size} bytes; limit {limit})")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error("{0}")]
    ReadOnly(String),
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
    #[error("{0}")]
    Shell(String),
    #[error("{0}")]
    Update(String),
    #[error("{0}")]
    Git(String),
    #[error("{0} already exists")]
    AlreadyExists(PathBuf),
    #[error("invalid path {0}")]
    InvalidPath(PathBuf),
    #[error("invalid key binding {entry}: {reason}")]
    InvalidBinding { entry: String, reason: String },
    #[error("{0}")]
    Cli(String),
    #[error("unhandled command {0}")]
    UnhandledCommand(&'static str),
}

impl Error {
    pub fn io(operation: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
