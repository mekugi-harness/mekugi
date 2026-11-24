use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::identity::ChangeId;

pub(crate) const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Provenance of a captured change record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSourceKind {
    /// Captured from stock `apply_patch` (direct, intercepted, or inside JS `exec`).
    ApplyPatch,
    /// Captured from shell-write observation (MEKUGI-801).
    ShellWrite,
    /// Captured from `mchanges revert`.
    MchangesRevert,
    /// Captured from `mchanges apply`.
    MchangesApply,
    /// Diagnostic VCS mutation (e.g. `git checkout`/`merge`); visible in `--history` only.
    VcsDiagnostic,
}

impl ChangeSourceKind {
    /// Whether this record counts as an authored edit (included in default review and counts).
    pub fn is_authored(self) -> bool {
        match self {
            Self::ApplyPatch | Self::ShellWrite | Self::MchangesRevert | Self::MchangesApply => {
                true
            }
            Self::VcsDiagnostic => false,
        }
    }

    /// Short human-readable label for CLI listings and history output.
    pub fn label(self) -> &'static str {
        match self {
            Self::ApplyPatch => "apply_patch",
            Self::ShellWrite => "shell_write",
            Self::MchangesRevert => "mchanges_revert",
            Self::MchangesApply => "mchanges_apply",
            Self::VcsDiagnostic => "vcs_diagnostic",
        }
    }
}

/// Honesty marker for per-file baseline/snapshot coverage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Partial { reason: String },
    MissingBaseline { reason: String },
}

impl CoverageStatus {
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Combine pre-edit and post-edit coverage honesty markers.
    pub fn merge(self, after: Self) -> Self {
        match (self, after) {
            (Self::Complete, Self::Complete) => Self::Complete,
            (Self::MissingBaseline { reason }, _) => Self::MissingBaseline { reason },
            (_, Self::MissingBaseline { reason }) => Self::Partial { reason },
            (Self::Partial { reason }, _) | (_, Self::Partial { reason }) => {
                Self::Partial { reason }
            }
        }
    }
}

/// Raw in-memory file bytes observed before or after an edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileSnapshot {
    Missing,
    Present(Vec<u8>),
}

impl FileSnapshot {
    pub fn is_binary(bytes: &[u8]) -> bool {
        let sample_len = bytes.len().min(8192);
        bytes[..sample_len].contains(&0) || std::str::from_utf8(bytes).is_err()
    }
}

/// Durable content-addressed reference to a persisted file snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileState {
    Missing,
    Present {
        blob_hash: String,
        byte_len: u64,
        is_binary: bool,
    },
}

/// High-level operation observed on a single path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileChangeKind {
    Add,
    Modify,
    Delete,
    Move { from: String },
}

/// Input describing one observed per-file change to persist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingFileChange {
    /// Display/filter path (workspace-relative with forward slashes when inside workspace).
    pub rel_path: String,
    /// Absolute filesystem path where the file was observed.
    pub abs_path: PathBuf,
    /// Optional previous relative path if this file was moved/renamed.
    pub moved_from: Option<String>,
    /// Pre-edit baseline bytes observed before the tool executed.
    pub before: FileSnapshot,
    /// Post-edit bytes observed after the tool executed.
    pub after: FileSnapshot,
    /// Coverage honesty marker for this file.
    pub coverage: CoverageStatus,
}

/// Immutable persisted per-file change evidence inside a [`ChangeRecord`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileEvidence {
    pub rel_path: String,
    pub abs_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    pub kind: FileChangeKind,
    pub before: FileState,
    pub after: FileState,
    pub coverage: CoverageStatus,
}

/// Attribution and context metadata for a [`ChangeRecord`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangeMetadata {
    pub thread_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub source_kind: ChangeSourceKind,
    pub workspace_root: PathBuf,
    /// Unix timestamp in seconds.
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Optional deduplication/claim key shared across patch and shell-write observers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_key: Option<String>,
}

/// Immutable published record for a single captured change event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub schema_version: u32,
    pub id: ChangeId,
    pub sequence: u64,
    pub metadata: ChangeMetadata,
    pub files: Vec<FileEvidence>,
}

/// Lightweight index entry stored in `index.json` for fast listing and lookup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangeIndexEntry {
    pub id: ChangeId,
    pub sequence: u64,
    pub thread_id: String,
    pub call_id: String,
    pub source_kind: ChangeSourceKind,
    pub created_at: i64,
    pub exit_code: Option<i32>,
    pub file_paths: Vec<String>,
    pub has_incomplete_coverage: bool,
}
