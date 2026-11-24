use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use similar::DiffOp;
use similar::TextDiff;

use crate::diff::compose_transitions;
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
        let transitions = compose_transitions(self, &records, path_filters)?;
        if transitions.is_empty() {
            return Err(ChangeHistoryError::NoMatchingPaths);
        }

        let workspace_root = records
            .last()
            .map(|r| r.metadata.workspace_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let mut planned = Vec::with_capacity(transitions.len());
        for tr in transitions {
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

            let merged = three_way_merge_strict(base, ours, theirs).ok_or_else(|| {
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditHunk<'a> {
    base_start: usize,
    base_end: usize,
    replacement: Vec<&'a str>,
}

fn collect_hunks<'a>(base_lines: &[&'a str], target_lines: &[&'a str]) -> Vec<EditHunk<'a>> {
    let diff = TextDiff::from_slices(base_lines, target_lines);
    let mut hunks = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => hunks.push(EditHunk {
                base_start: old_index,
                base_end: old_index + old_len,
                replacement: Vec::new(),
            }),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => hunks.push(EditHunk {
                base_start: old_index,
                base_end: old_index,
                replacement: target_lines[new_index..new_index + new_len].to_vec(),
            }),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => hunks.push(EditHunk {
                base_start: old_index,
                base_end: old_index + old_len,
                replacement: target_lines[new_index..new_index + new_len].to_vec(),
            }),
        }
    }
    hunks
}

fn hunks_overlap(a: &EditHunk<'_>, b: &EditHunk<'_>) -> bool {
    if a.base_start == a.base_end && b.base_start == b.base_end {
        return a.base_start == b.base_start;
    }
    if a.base_start == a.base_end {
        return a.base_start >= b.base_start && a.base_start <= b.base_end;
    }
    if b.base_start == b.base_end {
        return b.base_start >= a.base_start && b.base_start <= a.base_end;
    }
    a.base_start < b.base_end && b.base_start < a.base_end
}

/// Strict 3-way line merge: preserves non-overlapping edits from both sides and
/// returns `None` on any overlapping conflict without emitting conflict markers.
fn three_way_merge_strict(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let base_lines: Vec<&str> = base.split_inclusive('\n').collect();
    let ours_lines: Vec<&str> = ours.split_inclusive('\n').collect();
    let theirs_lines: Vec<&str> = theirs.split_inclusive('\n').collect();

    let ours_hunks = collect_hunks(&base_lines, &ours_lines);
    let theirs_hunks = collect_hunks(&base_lines, &theirs_lines);

    let mut out = String::new();
    let mut base_pos = 0usize;
    let mut i = 0usize;
    let mut j = 0usize;

    while i < ours_hunks.len() || j < theirs_hunks.len() {
        match (ours_hunks.get(i), theirs_hunks.get(j)) {
            (Some(oh), Some(th)) => {
                if hunks_overlap(oh, th) {
                    if oh == th {
                        for line in &base_lines[base_pos..oh.base_start] {
                            out.push_str(line);
                        }
                        for line in &oh.replacement {
                            out.push_str(line);
                        }
                        base_pos = oh.base_end;
                        i += 1;
                        j += 1;
                    } else {
                        return None;
                    }
                } else if (oh.base_start, oh.base_end) <= (th.base_start, th.base_end) {
                    for line in &base_lines[base_pos..oh.base_start] {
                        out.push_str(line);
                    }
                    for line in &oh.replacement {
                        out.push_str(line);
                    }
                    base_pos = oh.base_end;
                    i += 1;
                } else {
                    for line in &base_lines[base_pos..th.base_start] {
                        out.push_str(line);
                    }
                    for line in &th.replacement {
                        out.push_str(line);
                    }
                    base_pos = th.base_end;
                    j += 1;
                }
            }
            (Some(oh), None) => {
                for line in &base_lines[base_pos..oh.base_start] {
                    out.push_str(line);
                }
                for line in &oh.replacement {
                    out.push_str(line);
                }
                base_pos = oh.base_end;
                i += 1;
            }
            (None, Some(th)) => {
                for line in &base_lines[base_pos..th.base_start] {
                    out.push_str(line);
                }
                for line in &th.replacement {
                    out.push_str(line);
                }
                base_pos = th.base_end;
                j += 1;
            }
            (None, None) => break,
        }
    }

    for line in &base_lines[base_pos..] {
        out.push_str(line);
    }
    Some(out)
}
