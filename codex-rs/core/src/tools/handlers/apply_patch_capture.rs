//! Native tool-boundary change-evidence capture for `apply_patch`.
//! Captures pre-edit baseline bytes before patch execution and post-edit
//! observed bytes after execution (including partial writes before a failing
//! exit and patches executed inside JavaScript code-mode `exec` cells),
//! persisting durable evidence to `codex-change-history` before receipts are
//! published.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_apply_patch::AppliedPatchDelta;
use codex_apply_patch::AppliedPatchFileChange;
use codex_change_history::ChangeHistoryError;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeRecord;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::ReadFileOptions;
use codex_utils_path_uri::PathUri;

use crate::exec_env::change_history_dir;
use crate::tools::runtimes::apply_patch::ApplyPatchRequest;
use crate::tools::sandboxing::ToolCtx;

/// Pre-edit baseline capture held across an `apply_patch` execution.
pub(crate) struct ApplyPatchCapture {
    baselines: HashMap<PathUri, (FileSnapshot, CoverageStatus)>,
}

impl ApplyPatchCapture {
    /// Snapshot pre-edit bytes for every candidate target path before patch execution.
    pub(crate) async fn capture_baselines(
        fs: &dyn ExecutorFileSystem,
        candidate_paths: &[PathUri],
    ) -> Self {
        let mut baselines = HashMap::with_capacity(candidate_paths.len());
        for path in candidate_paths {
            if baselines.contains_key(path) {
                continue;
            }
            let snapshot = read_snapshot(fs, path).await;
            baselines.insert(path.clone(), snapshot);
        }
        Self { baselines }
    }

    /// Observe post-edit bytes and persist durable evidence into the session's
    /// native change-history store before publishing tool receipts.
    pub(crate) async fn persist_for_request(
        self,
        fs: &dyn ExecutorFileSystem,
        request: &ApplyPatchRequest,
        delta: Option<&AppliedPatchDelta>,
        exit_code: Option<i32>,
        tool_ctx: &ToolCtx,
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        let store_dir = change_history_dir(tool_ctx.session.session_id());
        let workspace_root = path_uri_to_path_buf(&request.action.cwd);
        let metadata = ChangeMetadata {
            thread_id: tool_ctx.session.thread_id().to_string(),
            session_id: tool_ctx.session.session_id().to_string(),
            turn_id: tool_ctx.step_context.turn.sub_id.clone(),
            call_id: tool_ctx.call_id.clone(),
            source_kind: ChangeSourceKind::ApplyPatch,
            workspace_root,
            created_at: unix_now_secs(),
            exit_code,
            summary: Some("apply_patch".to_string()),
            claim_key: None,
        };
        self.persist_to_dir(fs, &store_dir, &request.file_paths, delta, metadata)
            .await
    }

    /// Observe post-edit bytes and persist to `store_dir`.
    pub(crate) async fn persist_to_dir(
        self,
        fs: &dyn ExecutorFileSystem,
        store_dir: &Path,
        candidate_paths: &[PathUri],
        delta: Option<&AppliedPatchDelta>,
        metadata: ChangeMetadata,
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        let workspace_root = metadata.workspace_root.clone();
        let mut moved_from_by_dest: HashMap<PathUri, String> = HashMap::new();
        let mut all_paths: BTreeMap<String, PathUri> = BTreeMap::new();

        for path in candidate_paths {
            all_paths.insert(path.to_string(), path.clone());
        }
        if let Some(delta) = delta {
            for change in delta.changes() {
                all_paths.insert(change.path.to_string(), change.path.clone());
                if let AppliedPatchFileChange::Update {
                    move_path: Some(dest),
                    ..
                } = &change.change
                {
                    all_paths.insert(dest.to_string(), dest.clone());
                    let src_rel = display_rel_path(&workspace_root, &change.path);
                    moved_from_by_dest.insert(dest.clone(), src_rel);
                }
            }
        }

        let mut pending_files = Vec::new();
        for path in all_paths.values() {
            let (before, before_cov) = self.baselines.get(path).cloned().unwrap_or_else(|| {
                (
                    FileSnapshot::Missing,
                    CoverageStatus::MissingBaseline {
                        reason: "path was not in pre-edit candidate set".to_string(),
                    },
                )
            });
            let (after, after_cov) = read_snapshot(fs, path).await;
            let moved_from = moved_from_by_dest.get(path).cloned();

            if before == after && moved_from.is_none() {
                continue;
            }

            let coverage = merge_coverage(before_cov, after_cov);
            let rel_path = display_rel_path(&workspace_root, path);
            let abs_path = path_uri_to_path_buf(path);
            pending_files.push(PendingFileChange {
                rel_path,
                abs_path,
                moved_from,
                before,
                after,
                coverage,
            });
        }

        if pending_files.is_empty() {
            return Ok(None);
        }

        let store = ChangeHistoryStore::open(store_dir)?;
        let record = store.record_change(metadata, pending_files)?;
        Ok(Some(record))
    }
}

async fn read_snapshot(
    fs: &dyn ExecutorFileSystem,
    path: &PathUri,
) -> (FileSnapshot, CoverageStatus) {
    match fs
        .read_file(path, ReadFileOptions::default(), /*sandbox*/ None)
        .await
    {
        Ok(bytes) => (FileSnapshot::Present(bytes), CoverageStatus::Complete),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            (FileSnapshot::Missing, CoverageStatus::Complete)
        }
        Err(err) => (
            FileSnapshot::Missing,
            CoverageStatus::MissingBaseline {
                reason: err.to_string(),
            },
        ),
    }
}

fn merge_coverage(before: CoverageStatus, after: CoverageStatus) -> CoverageStatus {
    match (before, after) {
        (CoverageStatus::Complete, CoverageStatus::Complete) => CoverageStatus::Complete,
        (CoverageStatus::MissingBaseline { reason }, _) => {
            CoverageStatus::MissingBaseline { reason }
        }
        (_, CoverageStatus::MissingBaseline { reason }) => CoverageStatus::Partial { reason },
        (CoverageStatus::Partial { reason }, _) | (_, CoverageStatus::Partial { reason }) => {
            CoverageStatus::Partial { reason }
        }
    }
}

fn path_uri_to_path_buf(uri: &PathUri) -> PathBuf {
    uri.to_abs_path()
        .map(|abs| abs.to_path_buf())
        .unwrap_or_else(|_| PathBuf::from(uri.to_string()))
}

fn display_rel_path(workspace_root: &Path, uri: &PathUri) -> String {
    let abs = path_uri_to_path_buf(uri);
    let rel = abs.strip_prefix(workspace_root).unwrap_or(&abs);
    rel.to_string_lossy().replace('\\', "/")
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "apply_patch_capture_tests.rs"]
mod tests;
