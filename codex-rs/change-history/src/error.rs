use std::path::PathBuf;

use thiserror::Error;

/// Errors surfaced by the durable change-history store and `mchanges` operations.
#[derive(Debug, Error)]
pub enum ChangeHistoryError {
    #[error("change history storage failure: {0}")]
    Storage(String),

    #[error("invalid change ID `{0}`")]
    InvalidChangeId(String),

    #[error("invalid change ID range `{0}`: {1}")]
    InvalidChangeRange(String, String),

    #[error("unknown change ID `{0}`")]
    UnknownChangeId(String),

    #[error("missing change evidence for `{id}`: {detail}")]
    MissingEvidence { id: String, detail: String },

    #[error("corrupt change evidence for `{id}`: {detail}")]
    CorruptEvidence { id: String, detail: String },

    #[error("workspace drift detected for `{}`: {reason}", path.display())]
    DriftConflict { path: PathBuf, reason: String },

    #[error("no files matched the requested path filter")]
    NoMatchingPaths,

    #[error("at least one change ID is required")]
    EmptySelection,
}
