//! Bounded, provisional snapshots of regular files touched by a running command.

use std::io::ErrorKind;
use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use codex_change_history::FileSnapshot;
use codex_change_history::classify_change_scope;
use codex_change_history::render_provisional_text_diff;

use super::ObservationState;
use super::ReadPolicyFn;
use super::WriteProvenance;
use super::display_rel_path;
use super::open_observed_file;
use crate::active_writers::WriterKey;
use crate::active_writers::has_overlap;

const MAX_LIVE_PATHS: usize = 64;
const MAX_LIVE_BASELINE_PROBES: usize = 4 * MAX_LIVE_PATHS;
const MAX_LIVE_FILE_BYTES: usize = 64 * 1024;
const MAX_LIVE_INPUT_BYTES: usize = 256 * 1024;
const MAX_LIVE_DIFF_BYTES: usize = 256 * 1024;
const LIVE_DIFF_TIME: Duration = Duration::from_millis(50);

/// A reversible preview of an observed authored text change, not durable evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveFileChange {
    pub path: PathBuf,
    pub kind: LiveFileChangeKind,
    pub diff: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveFileChangeKind {
    Add,
    Delete,
    Update,
}

struct Candidate {
    path: PathBuf,
    before: Arc<FileSnapshot>,
    keys: Vec<WriterKey>,
}

pub(crate) struct LiveBaselines {
    candidates: Vec<Candidate>,
    call_id: String,
    can_read: ReadPolicyFn,
    remaining_bytes: usize,
}

impl ObservationState {
    pub(crate) fn has_live_candidates(&self) -> bool {
        !self.baselines.is_empty()
    }

    pub(crate) fn live_baselines(&self) -> LiveBaselines {
        // Leave at least half the read budget for current file contents.
        let mut remaining_baseline_bytes = MAX_LIVE_INPUT_BYTES / 2;
        let candidates = self
            .baselines
            .iter()
            // Bound time spent inspecting skipped baselines under the observer lock.
            .take(MAX_LIVE_BASELINE_PROBES)
            .filter_map(|(path, observed)| {
                if observed.provenance != WriteProvenance::Authored
                    || !observed.coverage.is_complete()
                {
                    return None;
                }
                let before = observed.before.as_ref()?;
                if let FileSnapshot::Present(bytes) = before.as_ref() {
                    if bytes.len() > MAX_LIVE_FILE_BYTES || bytes.len() > remaining_baseline_bytes {
                        return None;
                    }
                    remaining_baseline_bytes -= bytes.len();
                }
                Some(Candidate {
                    path: path.clone(),
                    before: Arc::clone(before),
                    keys: observed.keys.clone(),
                })
            })
            .take(MAX_LIVE_PATHS)
            .collect();
        LiveBaselines {
            candidates,
            call_id: self.call_id.clone(),
            can_read: Arc::clone(&self.can_read),
            remaining_bytes: MAX_LIVE_INPUT_BYTES / 2 + remaining_baseline_bytes,
        }
    }
}

impl LiveBaselines {
    pub(crate) fn render(
        self,
        workspace_root: &Path,
        session_tmp: Option<&Path>,
    ) -> Vec<LiveFileChange> {
        let mut changes = Vec::new();
        let mut remaining_diff_bytes = MAX_LIVE_DIFF_BYTES;
        let mut remaining_input_bytes = self.remaining_bytes;
        let deadline = Instant::now() + LIVE_DIFF_TIME;
        for candidate in self.candidates {
            if remaining_diff_bytes == 0 || remaining_input_bytes == 0 {
                break;
            }
            let Candidate { path, before, keys } = candidate;
            if !(self.can_read)(&path)
                || classify_change_scope(&path, workspace_root, session_tmp).is_ephemeral()
            {
                continue;
            }
            let Some(old) = snapshot_text(&before) else {
                continue;
            };
            let Some(after) = read_live_snapshot(&path, &mut remaining_input_bytes) else {
                continue;
            };
            if *before == after || has_overlap(&self.call_id, &keys) {
                continue;
            }
            let Some(new) = snapshot_text(&after) else {
                continue;
            };
            let kind = match (old, new) {
                (None, Some(_)) => LiveFileChangeKind::Add,
                (Some(_), None) => LiveFileChangeKind::Delete,
                (Some(_), Some(_)) => LiveFileChangeKind::Update,
                (None, None) => continue,
            };
            let display_path = display_rel_path(workspace_root, &path);
            let Some(diff) = render_provisional_text_diff(
                &display_path,
                old,
                new,
                remaining_diff_bytes,
                deadline,
            ) else {
                continue;
            };
            remaining_diff_bytes -= diff.len();
            changes.push(LiveFileChange { path, kind, diff });
        }
        changes
    }
}

fn snapshot_text(snapshot: &FileSnapshot) -> Option<Option<&str>> {
    match snapshot {
        FileSnapshot::Missing => Some(None),
        FileSnapshot::Present(bytes) => FileSnapshot::decode_text(bytes).map(Some),
    }
}

fn read_live_snapshot(path: &Path, remaining_bytes: &mut usize) -> Option<FileSnapshot> {
    let file = match open_observed_file(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Some(FileSnapshot::Missing);
        }
        Err(_) => return None,
    };
    // Include a byte to distinguish complete reads from a file that grew while sampling.
    let limit = (*remaining_bytes).min(MAX_LIVE_FILE_BYTES + 1);
    let opened = file.metadata().ok()?;
    if !opened.is_file() || opened.len() >= limit as u64 {
        return None;
    }

    let mut bytes = Vec::with_capacity(opened.len() as usize);
    let result = file.take(limit as u64).read_to_end(&mut bytes);
    *remaining_bytes -= bytes.len();
    if result.is_err() || bytes.len() == limit {
        return None;
    }
    Some(FileSnapshot::Present(bytes))
}
