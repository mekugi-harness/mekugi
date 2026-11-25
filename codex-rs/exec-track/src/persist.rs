//! Shared pre-write baseline state, path normalization, scope classification,
//! and `ChangeHistoryStore` persistence across Linux and macOS observers.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use codex_change_history::ChangeHistoryError;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeRecord;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use codex_change_history::classify_change_scope;

use crate::active_writers::InodeKey;
use crate::active_writers::finish_call;
use crate::active_writers::register_write;

/// Maximum inline file bytes read per file snapshot (8 MiB, matching change-evidence.md).
const MAX_CAPTURE_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) type ReadPolicyFn = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Mutable per-command write observation state shared between the OS supervisor
/// thread and final settlement.
pub(crate) struct ObservationState {
    call_id: String,
    ignored_root: PathBuf,
    can_read: ReadPolicyFn,
    baselines: BTreeMap<PathBuf, (FileSnapshot, CoverageStatus, Option<InodeKey>)>,
    partial_reasons: Vec<String>,
}

impl ObservationState {
    pub(crate) fn new(call_id: String, ignored_root: PathBuf, can_read: ReadPolicyFn) -> Self {
        let ignored_root = resolve_canonical_path(&ignored_root);
        Self {
            call_id,
            ignored_root,
            can_read,
            baselines: BTreeMap::new(),
            partial_reasons: Vec::new(),
        }
    }

    /// Lazily snapshot pre-write bytes the first time this command touches `path`.
    pub(crate) fn record_pre_write(&mut self, raw_path: &Path) {
        if should_ignore_path(raw_path, &self.ignored_root) {
            return;
        }
        let path = resolve_canonical_path(raw_path);
        if should_ignore_path(&path, &self.ignored_root) {
            return;
        }
        if self.baselines.contains_key(&path) {
            return;
        }
        if !(self.can_read)(&path) {
            return;
        }
        let inode = InodeKey::from_path(&path);
        let (snapshot, mut coverage) = read_bounded_snapshot(&path);
        if let Some(ino) = inode
            && register_write(&self.call_id, ino)
        {
            coverage = coverage.merge(CoverageStatus::Partial {
                reason: "concurrent write to same file".to_string(),
            });
        }
        self.baselines.insert(path, (snapshot, coverage, inode));
    }

    /// Record an explicit honesty marker (e.g., macOS SIP-protected binary executed).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn record_partial_reason(&mut self, reason: String) {
        if !self.partial_reasons.contains(&reason) {
            self.partial_reasons.push(reason);
        }
    }

    /// Release any held active-writer registrations without persisting.
    pub(crate) fn release_active_writers(&self) {
        let inodes: Vec<InodeKey> = self
            .baselines
            .values()
            .filter_map(|(_, _, ino)| *ino)
            .collect();
        if !inodes.is_empty() {
            let _ = finish_call(&self.call_id, &inodes);
        }
    }

    /// Observe post-execution bytes for all touched paths and persist durable
    /// evidence into `ChangeHistoryStore`.
    pub(crate) fn finalize_and_persist(
        &self,
        store_dir: &Path,
        workspace_root: &Path,
        session_tmp: Option<&Path>,
        metadata: ChangeMetadata,
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        let inodes: Vec<InodeKey> = self
            .baselines
            .values()
            .filter_map(|(_, _, ino)| *ino)
            .collect();
        let conflicted = finish_call(&self.call_id, &inodes);

        let mut pending_files = Vec::new();
        for (abs_path, (before, before_cov, inode)) in &self.baselines {
            if !(self.can_read)(abs_path)
                || fs::canonicalize(abs_path)
                    .ok()
                    .is_some_and(|canon| !(self.can_read)(&canon))
            {
                continue;
            }
            let (after, after_cov) = read_bounded_snapshot(abs_path);
            let mut coverage = before_cov.clone().merge(after_cov);
            if let Some(ino) = inode
                && conflicted.contains(ino)
            {
                coverage = coverage.merge(CoverageStatus::Partial {
                    reason: "concurrent write to same file".to_string(),
                });
            }
            if !self.partial_reasons.is_empty() {
                coverage = coverage.merge(CoverageStatus::Partial {
                    reason: self.partial_reasons.join("; "),
                });
            }
            if before == &after && coverage == CoverageStatus::Complete {
                continue;
            }
            let rel_path = display_rel_path(workspace_root, abs_path);
            let scope = classify_change_scope(abs_path, workspace_root, session_tmp);
            pending_files.push(PendingFileChange {
                rel_path,
                abs_path: abs_path.clone(),
                moved_from: None,
                before: before.clone(),
                after,
                coverage,
                scope,
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

fn should_ignore_path(path: &Path, ignored_root: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return true;
    }
    if path.starts_with(ignored_root) {
        return true;
    }
    let s = path.to_string_lossy();
    s == "/dev/null"
        || s == "/dev/tty"
        || s == "/dev/stdout"
        || s == "/dev/stderr"
        || s.starts_with("/dev/")
        || s.starts_with("/proc/")
        || s.starts_with("/sys/")
}

pub(crate) fn resolve_canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let mut resolved = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let candidate = resolved.join("..");
                if let Ok(canonical) = fs::canonicalize(&candidate) {
                    resolved = canonical;
                } else if !resolved.pop() {
                    resolved.push(comp.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                resolved.push(comp.as_os_str());
                if let Ok(canonical) = fs::canonicalize(&resolved) {
                    resolved = canonical;
                }
            }
        }
    }
    resolved
}

fn read_bounded_snapshot(path: &Path) -> (FileSnapshot, CoverageStatus) {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return (FileSnapshot::Missing, CoverageStatus::Complete);
        }
        Err(err) => {
            return (
                FileSnapshot::Missing,
                CoverageStatus::MissingBaseline {
                    reason: err.to_string(),
                },
            );
        }
    };
    if !meta.file_type().is_file() {
        return (FileSnapshot::Missing, CoverageStatus::Complete);
    }
    if meta.len() > MAX_CAPTURE_FILE_BYTES {
        return (
            FileSnapshot::Missing,
            CoverageStatus::Partial {
                reason: format!(
                    "file exceeds {MAX_CAPTURE_FILE_BYTES}-byte capture limit ({} bytes)",
                    meta.len()
                ),
            },
        );
    }
    match fs::read(path) {
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

fn display_rel_path(workspace_root: &Path, abs: &Path) -> String {
    let rel = abs.strip_prefix(workspace_root).unwrap_or_else(|_| {
        if let Ok(canonical_root) = fs::canonicalize(workspace_root) {
            abs.strip_prefix(&canonical_root).unwrap_or(abs)
        } else {
            abs
        }
    });
    rel.to_string_lossy().replace('\\', "/")
}
