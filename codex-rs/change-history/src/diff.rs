use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use similar::ChangeTag;
use similar::TextDiff;

use crate::error::ChangeHistoryError;
use crate::identity::ChangeId;
use crate::model::ChangeRecord;
use crate::model::CoverageStatus;
use crate::model::FileSnapshot;
use crate::store::ChangeHistoryStore;

/// Per-file statistics in a composed summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiffStats {
    pub rel_path: String,
    pub moved_from: Option<String>,
    pub status: char,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    pub coverage_notes: Vec<String>,
}

/// Composed summary across one or more [`ChangeRecord`]s.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedSummary {
    pub ids: Vec<ChangeId>,
    pub files: Vec<FileDiffStats>,
    pub total_additions: usize,
    pub total_deletions: usize,
}

/// Composed diff across one or more [`ChangeRecord`]s.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedFileDiff {
    pub rel_path: String,
    pub moved_from: Option<String>,
    pub unified_diff: String,
    pub stats: FileDiffStats,
}

pub(crate) struct ComposedFileTransition {
    pub(crate) rel_path: String,
    pub(crate) abs_path: PathBuf,
    pub(crate) moved_from: Option<String>,
    pub(crate) before: FileSnapshot,
    pub(crate) after: FileSnapshot,
    pub(crate) coverage_notes: Vec<String>,
}

pub(crate) fn normalize_filter_path(path: &str) -> String {
    let trimmed = path.trim().trim_start_matches("./");
    trimmed.replace('\\', "/")
}

pub(crate) fn path_matches_filters(rel_path: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let norm_rel = normalize_filter_path(rel_path);
    filters.iter().any(|filter| {
        let norm_filter = normalize_filter_path(filter);
        let norm_filter = norm_filter.trim_end_matches('/');
        norm_rel == norm_filter
            || norm_rel
                .strip_prefix(norm_filter)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn normalize_abs_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(comp.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(comp.as_os_str());
            }
        }
    }
    out
}

/// Resolve user-supplied path_filters into exact normalized absolute paths,
/// expanding rename counterparts (including chains) within each record's own
/// workspace root so a rename in pkg/a never matches a same-named file in pkg/b.
fn select_matching_abs_paths(
    records: &[&ChangeRecord],
    path_filters: &[String],
) -> Option<BTreeSet<PathBuf>> {
    if path_filters.is_empty() {
        return None;
    }
    let mut matched = BTreeSet::new();
    for record in records {
        for file in &record.files {
            let dest_abs = normalize_abs_path(&file.abs_path);
            if path_matches_filters(&file.rel_path, path_filters) {
                matched.insert(dest_abs.clone());
            }
            if let Some(from) = file.moved_from.as_deref()
                && path_matches_filters(from, path_filters)
            {
                matched.insert(dest_abs);
                matched.insert(normalize_abs_path(&record.metadata.workspace_root.join(from)));
            }
        }
    }
    loop {
        let mut added = false;
        for record in records {
            for file in &record.files {
                let Some(from) = file.moved_from.as_deref() else {
                    continue;
                };
                let dest_abs = normalize_abs_path(&file.abs_path);
                let src_abs = normalize_abs_path(&record.metadata.workspace_root.join(from));
                if (matched.contains(&dest_abs) && matched.insert(src_abs.clone()))
                    || (matched.contains(&src_abs) && matched.insert(dest_abs))
                {
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }
    Some(matched)
}

pub(crate) fn compose_transitions(
    store: &ChangeHistoryStore,
    records: &[ChangeRecord],
    path_filters: &[String],
) -> Result<Vec<ComposedFileTransition>, ChangeHistoryError> {
    compose_transitions_with_continuity(store, records, path_filters, false)
}

pub(crate) fn compose_transitions_with_continuity(
    store: &ChangeHistoryStore,
    records: &[ChangeRecord],
    path_filters: &[String],
    require_continuity: bool,
) -> Result<Vec<ComposedFileTransition>, ChangeHistoryError> {
    let mut ordered: Vec<&ChangeRecord> = records.iter().collect();
    ordered.sort_by_key(|r| r.sequence);
    let matched_abs = select_matching_abs_paths(&ordered, path_filters);

    let mut by_path: BTreeMap<PathBuf, (u64, ComposedFileTransition)> = BTreeMap::new();
    for record in ordered {
        for file in &record.files {
            let key = normalize_abs_path(&file.abs_path);
            if let Some(allowed) = &matched_abs
                && !allowed.contains(&key)
            {
                continue;
            }
            let before = store.read_file_state(&record.id, &file.before)?;
            let after = store.read_file_state(&record.id, &file.after)?;
            let note = match &file.coverage {
                CoverageStatus::Complete => None,
                CoverageStatus::Partial { reason } => {
                    Some(format!("{}: partial ({reason})", record.id))
                }
                CoverageStatus::MissingBaseline { reason } => {
                    Some(format!("{}: missing baseline ({reason})", record.id))
                }
            };
            if let Some((last_seq, existing)) = by_path.get_mut(&key) {
                if *last_seq == record.sequence {
                    // Duplicate path spelling within the same record: coalesce without
                    // treating the two snapshots as sequential edits.
                    if let Some(note) = note {
                        existing.coverage_notes.push(note);
                    }
                    continue;
                }
                if require_continuity && existing.after != before {
                    return Err(ChangeHistoryError::DriftConflict {
                        path: file.abs_path.clone(),
                        reason: format!(
                            "discontinuous selection for `{}`: skipped intermediate edits before `{}`",
                            file.rel_path, record.id
                        ),
                    });
                }
                *last_seq = record.sequence;
                existing.after = after;
                existing.rel_path = file.rel_path.clone();
                existing.abs_path = file.abs_path.clone();
                if existing.moved_from.is_none() {
                    existing.moved_from = file.moved_from.clone();
                }
                if let Some(note) = note {
                    existing.coverage_notes.push(note);
                }
            } else {
                let mut coverage_notes = Vec::new();
                if let Some(note) = note {
                    coverage_notes.push(note);
                }
                by_path.insert(
                    key,
                    (
                        record.sequence,
                        ComposedFileTransition {
                            rel_path: file.rel_path.clone(),
                            abs_path: file.abs_path.clone(),
                            moved_from: file.moved_from.clone(),
                            before,
                            after,
                            coverage_notes,
                        },
                    ),
                );
            }
        }
    }

    Ok(by_path.into_values().map(|(_, tr)| tr).collect())
}

/// Compose unified diffs across `records` (in capture order), optionally filtered by `path_filters`.
pub fn compose_diff(
    store: &ChangeHistoryStore,
    records: &[ChangeRecord],
    path_filters: &[String],
) -> Result<Vec<ComposedFileDiff>, ChangeHistoryError> {
    let transitions = compose_transitions(store, records, path_filters)?;
    let mut out = Vec::with_capacity(transitions.len());
    for tr in transitions {
        let (unified_diff, stats) = render_transition_diff(&tr);
        out.push(ComposedFileDiff {
            rel_path: tr.rel_path,
            moved_from: tr.moved_from,
            unified_diff,
            stats,
        });
    }
    Ok(out)
}

/// Compute a composed summary across `records`, optionally filtered by `path_filters`.
pub fn compose_summary(
    store: &ChangeHistoryStore,
    records: &[ChangeRecord],
    path_filters: &[String],
) -> Result<ComposedSummary, ChangeHistoryError> {
    let diffs = compose_diff(store, records, path_filters)?;
    let total_additions = diffs.iter().map(|d| d.stats.additions).sum();
    let total_deletions = diffs.iter().map(|d| d.stats.deletions).sum();
    let files = diffs.into_iter().map(|d| d.stats).collect();
    let ids = records.iter().map(|r| r.id.clone()).collect();
    Ok(ComposedSummary {
        ids,
        files,
        total_additions,
        total_deletions,
    })
}

/// Render chronological per-record history (including diagnostic records and exit codes).
pub fn render_history(records: &[ChangeRecord], path_filters: &[String]) -> String {
    let mut ordered: Vec<&ChangeRecord> = records.iter().collect();
    ordered.sort_by_key(|r| r.sequence);
    let matched_abs = select_matching_abs_paths(&ordered, path_filters);

    let mut out = String::new();
    for record in ordered {
        let matched_files: Vec<_> = record
            .files
            .iter()
            .filter(|f| match &matched_abs {
                Some(allowed) => allowed.contains(&normalize_abs_path(&f.abs_path)),
                None => true,
            })
            .collect();
        if !path_filters.is_empty() && matched_files.is_empty() {
            continue;
        }
        let exit_str = match record.metadata.exit_code {
            Some(code) => format!(" exit={code}"),
            None => String::new(),
        };
        let _ = writeln!(
            &mut out,
            "{} [{}] thread={} call={}{}",
            record.id,
            record.metadata.source_kind.label(),
            record.metadata.thread_id,
            record.metadata.call_id,
            exit_str
        );
        if let Some(summary) = &record.metadata.summary {
            let _ = writeln!(&mut out, "  summary: {summary}");
        }
        for file in matched_files {
            let coverage_suffix = match &file.coverage {
                CoverageStatus::Complete => String::new(),
                CoverageStatus::Partial { reason } => format!(" [partial: {reason}]"),
                CoverageStatus::MissingBaseline { reason } => {
                    format!(" [missing baseline: {reason}]")
                }
            };
            let _ = writeln!(
                &mut out,
                "  {:?} {}{}",
                file.kind, file.rel_path, coverage_suffix
            );
        }
    }
    out
}

fn render_transition_diff(tr: &ComposedFileTransition) -> (String, FileDiffStats) {
    let status = if tr.moved_from.is_some() {
        'R'
    } else {
        match (&tr.before, &tr.after) {
            (FileSnapshot::Missing, FileSnapshot::Present(_)) => 'A',
            (FileSnapshot::Present(_), FileSnapshot::Missing) => 'D',
            (FileSnapshot::Present(_), FileSnapshot::Present(_))
            | (FileSnapshot::Missing, FileSnapshot::Missing) => 'M',
        }
    };

    let is_binary = matches!(&tr.before, FileSnapshot::Present(b) if FileSnapshot::is_binary(b))
        || matches!(&tr.after, FileSnapshot::Present(b) if FileSnapshot::is_binary(b));

    let mut out = String::new();
    let from_header = tr.moved_from.as_deref().unwrap_or(tr.rel_path.as_str());
    let _ = writeln!(&mut out, "diff --git a/{from_header} b/{}", tr.rel_path);
    if let Some(from) = &tr.moved_from {
        let _ = writeln!(&mut out, "rename from {from}");
        let _ = writeln!(&mut out, "rename to {}", tr.rel_path);
    }
    for note in &tr.coverage_notes {
        let _ = writeln!(&mut out, "# coverage: {note}");
    }

    if is_binary {
        let old_label = match &tr.before {
            FileSnapshot::Missing => "/dev/null".to_string(),
            FileSnapshot::Present(_) => format!("a/{from_header}"),
        };
        let new_label = match &tr.after {
            FileSnapshot::Missing => "/dev/null".to_string(),
            FileSnapshot::Present(_) => format!("b/{}", tr.rel_path),
        };
        let _ = writeln!(&mut out, "Binary files {old_label} and {new_label} differ");
        return (
            out,
            FileDiffStats {
                rel_path: tr.rel_path.clone(),
                moved_from: tr.moved_from.clone(),
                status,
                additions: 0,
                deletions: 0,
                is_binary: true,
                coverage_notes: tr.coverage_notes.clone(),
            },
        );
    }

    let old_text = match &tr.before {
        FileSnapshot::Missing => "",
        FileSnapshot::Present(bytes) => std::str::from_utf8(bytes).unwrap_or_default(),
    };
    let new_text = match &tr.after {
        FileSnapshot::Missing => "",
        FileSnapshot::Present(bytes) => std::str::from_utf8(bytes).unwrap_or_default(),
    };

    let diff = TextDiff::from_lines(old_text, new_text);
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => deletions += 1,
            ChangeTag::Equal => {}
        }
    }

    let old_header = match &tr.before {
        FileSnapshot::Missing => "/dev/null".to_string(),
        FileSnapshot::Present(_) => format!("a/{from_header}"),
    };
    let new_header = match &tr.after {
        FileSnapshot::Missing => "/dev/null".to_string(),
        FileSnapshot::Present(_) => format!("b/{}", tr.rel_path),
    };
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&old_header, &new_header)
        .to_string();
    out.push_str(&unified);

    (
        out,
        FileDiffStats {
            rel_path: tr.rel_path.clone(),
            moved_from: tr.moved_from.clone(),
            status,
            additions,
            deletions,
            is_binary: false,
            coverage_notes: tr.coverage_notes.clone(),
        },
    )
}
