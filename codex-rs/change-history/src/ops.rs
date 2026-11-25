use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::diff::compose_transitions_with_continuity;
use crate::error::ChangeHistoryError;
use crate::identity::ChangeId;
use crate::model::ChangeMetadata;
use crate::model::ChangeRecord;
use crate::model::ChangeSourceKind;
use crate::model::CoverageStatus;
use crate::model::FileSnapshot;
use crate::model::PendingFileChange;
use crate::store::ChangeHistoryStore;

/// Direction for mutating the workspace from retained change evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationDirection {
    /// Undo selected changes (latest-first).
    Revert,
    /// Reapply selected changes (capture-order).
    Apply,
}

/// Result of a successful `revert` or `apply` operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationOutcome {
    /// The newly persisted change record capturing this revert/apply.
    pub recorded: ChangeRecord,
    /// Workspace-relative paths written or deleted.
    pub affected_paths: Vec<String>,
}

struct PlannedWrite {
    rel_path: String,
    abs_path: PathBuf,
    live_before: FileSnapshot,
    target_after: FileSnapshot,
}

impl ChangeHistoryStore {
    /// Revert or reapply the selected `ids` (optionally scoped to `path_filters`).
    ///
    /// All referenced records and blobs are verified and all target files are
    /// merged/validated in memory before any file is written to disk. On drift,
    /// missing evidence, or corrupt evidence, fails explicitly without writing
    /// conflict markers or partially mutating the workspace.
    pub fn mutate_workspace(
        &self,
        direction: MutationDirection,
        ids: &[ChangeId],
        path_filters: &[String],
        actor_thread_id: &str,
        actor_session_id: &str,
    ) -> Result<MutationOutcome, ChangeHistoryError> {
        let records = self.load_records(ids)?;
        let transitions = compose_transitions_with_continuity(self, &records, path_filters, true)?;
        if transitions.is_empty() {
            return Err(ChangeHistoryError::NoMatchingPaths);
        }

        let workspace_root = records
            .last()
            .map(|r| r.metadata.workspace_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let mut planned = Vec::with_capacity(transitions.len());
        for tr in transitions {
            if tr.before == tr.after && tr.moved_from.is_none() && tr.coverage_notes.is_empty() {
                continue;
            }
            if !tr.coverage_notes.is_empty() {
                return Err(ChangeHistoryError::MissingEvidence {
                    id: tr.rel_path.clone(),
                    detail: tr.coverage_notes.join("; "),
                });
            }
            let (expected, desired) = match direction {
                MutationDirection::Revert => (tr.after, tr.before),
                MutationDirection::Apply => (tr.before, tr.after),
            };
            let live_before = read_live_snapshot(&tr.abs_path)?;
            let target_after =
                resolve_target_snapshot(&tr.abs_path, &expected, &desired, &live_before)?;
            planned.push(PlannedWrite {
                rel_path: tr.rel_path,
                abs_path: tr.abs_path,
                live_before,
                target_after,
            });
        }

        // Commit validated writes to disk.
        let mut pending_files = Vec::with_capacity(planned.len());
        let mut affected_paths = Vec::with_capacity(planned.len());
        for write in planned {
            apply_snapshot_to_disk(&write.abs_path, &write.target_after)?;
            affected_paths.push(write.rel_path.clone());
            pending_files.push(PendingFileChange {
                rel_path: write.rel_path,
                abs_path: write.abs_path,
                moved_from: None,
                before: write.live_before,
                after: write.target_after,
                coverage: CoverageStatus::Complete,
            });
        }

        let source_kind = match direction {
            MutationDirection::Revert => ChangeSourceKind::MchangesRevert,
            MutationDirection::Apply => ChangeSourceKind::MchangesApply,
        };
        let id_list = ids
            .iter()
            .map(ChangeId::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let verb = match direction {
            MutationDirection::Revert => "revert",
            MutationDirection::Apply => "apply",
        };
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let recorded = self.record_change(
            ChangeMetadata {
                thread_id: actor_thread_id.to_string(),
                session_id: actor_session_id.to_string(),
                turn_id: String::new(),
                call_id: String::new(),
                source_kind,
                workspace_root,
                created_at,
                exit_code: Some(0),
                summary: Some(format!("mchanges {verb} {id_list}")),
                claim_key: None,
            },
            pending_files,
        )?;

        Ok(MutationOutcome {
            recorded,
            affected_paths,
        })
    }
}

fn read_live_snapshot(path: &Path) -> Result<FileSnapshot, ChangeHistoryError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "live path is a symlink; refusing to follow or overwrite".to_string(),
                });
            }
            if !meta.file_type().is_file() {
                return Err(ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "live path is not a regular file".to_string(),
                });
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(FileSnapshot::Missing),
        Err(err) => {
            return Err(ChangeHistoryError::Storage(format!(
                "stat workspace file {}: {err}",
                path.display()
            )));
        }
    }
    match fs::read(path) {
        Ok(bytes) => Ok(FileSnapshot::Present(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(FileSnapshot::Missing),
        Err(err) => Err(ChangeHistoryError::Storage(format!(
            "read workspace file {}: {err}",
            path.display()
        ))),
    }
}

fn apply_snapshot_to_disk(path: &Path, snapshot: &FileSnapshot) -> Result<(), ChangeHistoryError> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && (meta.file_type().is_symlink() || !meta.file_type().is_file())
    {
        return Err(ChangeHistoryError::DriftConflict {
            path: path.to_path_buf(),
            reason: "refusing to overwrite symlink or non-regular file".to_string(),
        });
    }
    match snapshot {
        FileSnapshot::Missing => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(ChangeHistoryError::Storage(format!(
                "delete {}: {err}",
                path.display()
            ))),
        },
        FileSnapshot::Present(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|err| {
                    ChangeHistoryError::Storage(format!(
                        "create parent dir {}: {err}",
                        parent.display()
                    ))
                })?;
            }
            fs::write(path, bytes).map_err(|err| {
                ChangeHistoryError::Storage(format!("write {}: {err}", path.display()))
            })
        }
    }
}

fn resolve_target_snapshot(
    path: &Path,
    expected: &FileSnapshot,
    desired: &FileSnapshot,
    live: &FileSnapshot,
) -> Result<FileSnapshot, ChangeHistoryError> {
    if live == expected {
        return Ok(desired.clone());
    }

    match (expected, desired, live) {
        // Creation deletion (expected Present, desired Missing) with drifted or missing live file.
        (FileSnapshot::Present(_), FileSnapshot::Missing, FileSnapshot::Present(_)) => {
            Err(ChangeHistoryError::DriftConflict {
                path: path.to_path_buf(),
                reason: "file was modified after creation; refusing to delete drifted content"
                    .to_string(),
            })
        }
        (FileSnapshot::Present(_), FileSnapshot::Missing, FileSnapshot::Missing) => {
            Err(ChangeHistoryError::DriftConflict {
                path: path.to_path_buf(),
                reason: "file is already missing from workspace".to_string(),
            })
        }
        // Deletion restoration / creation reapply (expected Missing, desired Present) when file already exists.
        (FileSnapshot::Missing, FileSnapshot::Present(_), FileSnapshot::Present(_)) => {
            Err(ChangeHistoryError::DriftConflict {
                path: path.to_path_buf(),
                reason: "file already exists in workspace with different content".to_string(),
            })
        }
        // Modification when live file was deleted externally.
        (FileSnapshot::Present(_), FileSnapshot::Present(_), FileSnapshot::Missing) => {
            Err(ChangeHistoryError::DriftConflict {
                path: path.to_path_buf(),
                reason: "file is missing from workspace".to_string(),
            })
        }
        // Modification with live workspace edits: attempt clean non-overlapping 3-way merge.
        (
            FileSnapshot::Present(base_bytes),
            FileSnapshot::Present(theirs_bytes),
            FileSnapshot::Present(ours_bytes),
        ) => {
            if FileSnapshot::is_binary(base_bytes)
                || FileSnapshot::is_binary(theirs_bytes)
                || FileSnapshot::is_binary(ours_bytes)
            {
                return Err(ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "binary file has drifted from expected snapshot".to_string(),
                });
            }
            let base =
                std::str::from_utf8(base_bytes).map_err(|_| ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "non-UTF-8 baseline cannot be merged".to_string(),
                })?;
            let theirs = std::str::from_utf8(theirs_bytes).map_err(|_| {
                ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "non-UTF-8 target cannot be merged".to_string(),
                }
            })?;
            let ours =
                std::str::from_utf8(ours_bytes).map_err(|_| ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "non-UTF-8 workspace file cannot be merged".to_string(),
                })?;

            let merged = diffy::merge(base, ours, theirs).map_err(|_| {
                ChangeHistoryError::DriftConflict {
                    path: path.to_path_buf(),
                    reason: "overlapping edits conflict with workspace changes".to_string(),
                }
            })?;
            Ok(FileSnapshot::Present(merged.into_bytes()))
        }
        // Missing -> Missing with live Present.
        (FileSnapshot::Missing, FileSnapshot::Missing, FileSnapshot::Present(_)) => {
            Err(ChangeHistoryError::DriftConflict {
                path: path.to_path_buf(),
                reason: "unexpected file present in workspace".to_string(),
            })
        }
        // Handled by `live == expected` above.
        (FileSnapshot::Missing, FileSnapshot::Present(_), FileSnapshot::Missing)
        | (FileSnapshot::Missing, FileSnapshot::Missing, FileSnapshot::Missing) => {
            Ok(desired.clone())
        }
    }
}
