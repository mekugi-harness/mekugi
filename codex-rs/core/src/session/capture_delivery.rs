//! Native retained records are immutable. A sampled, complete history item is
//! the receipt for a change, not the worker's return or an unprinted result.

use super::Session;
use crate::agent_communication::FinalCaptureAttachment;
use codex_change_history::{
    ChangeHistoryStore, ChangeId, ChangeIndexEntry, ChangeRecord, ChangeScope, FileState,
    compose_diff, expand_id_spec,
};
use codex_history::ResponseItemEnvelope;
use codex_protocol::agent_capture::{CaptureCursor, CaptureInterval};
use codex_protocol::{SessionId, ThreadId};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

#[derive(Clone)]
pub(crate) struct ProgressActor {
    pub(crate) thread_id: ThreadId,
    pub(crate) task_name: String,
}

pub(crate) struct ProgressCapture {
    pub(crate) actor: ProgressActor,
    pub(crate) entries: Vec<ChangeIndexEntry>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct ProgressOutput {
    pub(crate) text: String,
    pub(crate) delivered_intervals: Vec<CaptureInterval>,
}

pub(crate) fn interval(id: &ChangeId, sequence: u64) -> CaptureInterval {
    CaptureInterval {
        key: format!("native:{id}"),
        change_id: id.to_string(),
        record_order: sequence,
        legacy_record_key: None,
        first_chunk: 1,
        last_chunk: 1,
    }
}

#[tracing::instrument(skip_all)]
pub(crate) async fn read_change_source(
    session: SessionId,
    selection: Vec<String>,
    recipient: Option<ThreadId>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let store =
            ChangeHistoryStore::open_existing(crate::exec_env::change_history_dir(session))?;
        let ids = selection
            .iter()
            .map(|spec| expand_id_spec(spec))
            .collect::<Result<Vec<_>, _>>()?;
        let mut records = store.load_records(&ids.into_iter().flatten().collect::<Vec<_>>())?;
        if let Some(recipient) = recipient {
            let recipient = recipient.to_string();
            records.retain(|record| {
                !record.metadata.is_unattributed()
                    && record.metadata.thread_id != recipient
                    && record.metadata.source_kind.is_authored()
            });
        }
        for record in &mut records {
            record
                .files
                .retain(|file| file.scope != ChangeScope::Ephemeral);
        }
        records.retain(|record| !record.files.is_empty());
        render_composed_change_source(&store, &records, crate::context_sources::MAX_SOURCE_BYTES)?
            .ok_or_else(|| anyhow::anyhow!("source exceeded the retained change attachment budget"))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
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

fn render_composed_change_source(
    store: &ChangeHistoryStore,
    records: &[ChangeRecord],
    limit: usize,
) -> anyhow::Result<Option<String>> {
    let mut ordered: Vec<&ChangeRecord> = records.iter().collect();
    ordered.sort_by_key(|record| record.sequence);

    let mut body = String::new();
    if ordered
        .iter()
        .any(|record| record.files.iter().any(|file| !file.coverage.is_complete()))
    {
        let notice = "Incomplete capture coverage.\n";
        if notice.len() > limit {
            return Ok(None);
        }
        body.push_str(notice);
    }

    // Compose consecutive per-file transitions whose retained endpoints match so
    // superseded intermediate text is omitted, without folding gaps from skipped
    // records into the selected edits.
    let mut by_path: BTreeMap<PathBuf, Vec<(u64, FileState, Vec<ChangeRecord>)>> = BTreeMap::new();
    for record in ordered {
        for file in &record.files {
            let key = normalize_abs_path(&file.abs_path);
            let single_file = ChangeRecord {
                schema_version: record.schema_version,
                id: record.id.clone(),
                sequence: record.sequence,
                recorded_at_ms: record.recorded_at_ms,
                metadata: record.metadata.clone(),
                files: vec![file.clone()],
            };
            let runs = by_path.entry(key).or_default();
            if let Some((last_seq, last_after, run)) = runs.last_mut() {
                if *last_seq == record.sequence {
                    run.push(single_file);
                } else if file.moved_from.is_none()
                    && run.last().is_none_or(|prev| {
                        prev.files
                            .iter()
                            .all(|prev_file| prev_file.moved_from.is_none())
                    })
                    && *last_after == file.before
                {
                    *last_seq = record.sequence;
                    *last_after = file.after.clone();
                    run.push(single_file);
                } else {
                    runs.push((record.sequence, file.after.clone(), vec![single_file]));
                }
            } else {
                runs.push((record.sequence, file.after.clone(), vec![single_file]));
            }
        }
    }

    for runs in by_path.into_values() {
        for (_, _, run) in runs {
            for diff in compose_diff(store, &run, &[])? {
                if body.len().saturating_add(diff.unified_diff.len()) > limit {
                    return Ok(None);
                }
                body.push_str(&diff.unified_diff);
            }
        }
    }
    Ok(Some(body))
}

fn render_change_record(
    store: &ChangeHistoryStore,
    record: &ChangeRecord,
    limit: usize,
) -> anyhow::Result<Option<String>> {
    let mut body = String::new();
    if record.files.iter().any(|file| !file.coverage.is_complete()) {
        let notice = "Incomplete capture coverage.\n";
        if notice.len() > limit {
            return Ok(None);
        }
        body.push_str(notice);
    }
    for diff in compose_diff(store, std::slice::from_ref(record), &[])? {
        if body.len().saturating_add(diff.unified_diff.len()) > limit {
            return Ok(None);
        }
        body.push_str(&diff.unified_diff);
    }
    Ok(Some(body))
}

impl FinalCaptureAttachment {
    pub(crate) async fn freeze(
        session: SessionId,
        recipient: ThreadId,
        sources: Vec<String>,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        tokio::task::spawn_blocking(move || {
            let store =
                ChangeHistoryStore::open_existing(crate::exec_env::change_history_dir(session))?;
            let entries = store.list_entries()?;
            let index: HashMap<_, _> = entries.iter().map(|entry| (&entry.id, entry)).collect();
            let mut changes = Vec::new();
            let mut diagnostics = Vec::new();
            let recipient = recipient.to_string();
            for source in sources {
                let selection = source
                    .strip_prefix("mchanges:")
                    .ok_or_else(|| anyhow::anyhow!("invalid change source: {source}"))?;
                for spec in selection.split(',') {
                    let ids = match expand_id_spec(spec) {
                        Ok(ids) => ids,
                        Err(error) => {
                            diagnostics.push(error.to_string());
                            continue;
                        }
                    };
                    let mut first_missing = None;
                    for id in ids {
                        if let Some(entry) = index.get(&id) {
                            if !entry.is_unattributed()
                                && !entry.ephemeral_only
                                && entry.thread_id != recipient
                                && entry.source_kind.is_authored()
                            {
                                changes.push(interval(&id, entry.sequence));
                            }
                        } else if first_missing.is_none() {
                            first_missing = Some(id);
                        }
                    }
                    // Bound the diagnostic by the selection, not by a missing range's length.
                    if let Some(id) = first_missing {
                        diagnostics.push(format!("mchanges:{spec}: unknown change ID {id}"));
                    }
                }
            }
            changes.sort_unstable_by_key(|i| i.record_order);
            changes.dedup_by_key(|i| i.record_order);
            Ok((
                Self {
                    session_id: session,
                    changes,
                },
                diagnostics,
            ))
        })
        .await?
    }

    pub(crate) async fn project(&self, seen: &[CaptureCursor]) -> (String, Vec<CaptureInterval>) {
        let seen: HashSet<_> = seen.iter().map(|cursor| &cursor.key).collect();
        let intervals: Vec<_> = self
            .changes
            .iter()
            .filter(|i| !seen.contains(&i.key))
            .cloned()
            .collect();
        if intervals.is_empty() {
            return (String::new(), intervals);
        }
        let selection = intervals
            .iter()
            .map(|i| i.change_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let dir = crate::exec_env::change_history_dir(self.session_id);
        let ids: Result<Vec<ChangeId>, _> = intervals.iter().map(|i| i.change_id.parse()).collect();
        let rendered = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
            let store = ChangeHistoryStore::open_existing(dir)?;
            let records = store.load_records(&ids?)?;
            let mut text = String::new();
            for record in &records {
                let Some(body) =
                    render_change_record(&store, record, 16_000usize.saturating_sub(text.len()))?
                else {
                    return Ok(None);
                };
                let source = format!("mchanges:{}", record.id);
                let frame = crate::context_sources::frame_source_body(&source, &body)?;
                if text.len() + frame.len() + 1 > 16_000 {
                    return Ok(None);
                }
                writeln!(text, "{frame}")?;
            }
            text.pop();
            Ok(Some(text))
        })
        .await;
        let source = format!("mchanges:{selection}");
        match rendered
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        {
            Ok(Some(text)) => (text, intervals),
            Ok(None) => (
                format!("Captured changes exceed the attachment budget. Read {source}."),
                Vec::new(),
            ),
            Err(error) => (
                format!("{source}: CONTENT NOT ATTACHED: {error}"),
                Vec::new(),
            ),
        }
    }
}

impl Session {
    pub(crate) async fn capture_context(&self) -> (String, Vec<CaptureCursor>) {
        let state = self.state.lock().await;
        let context_id = self.current_window_from_state(&state).2.to_string();
        let seen = delivered_cursors(state.history.annotated_items(), &context_id);
        (context_id, seen)
    }

    pub(crate) async fn select_progress_ranges(
        &self,
        selected: Vec<ProgressActor>,
    ) -> anyhow::Result<Vec<ProgressCapture>> {
        let (_, seen) = self.capture_context().await;
        let dir = crate::exec_env::change_history_dir(self.session_id());
        tokio::task::spawn_blocking(move || {
            if !dir.try_exists()? {
                return Ok(Vec::new());
            }
            let seen: HashSet<_> = seen.into_iter().map(|cursor| cursor.key).collect();
            let mut agents: Vec<_> = selected
                .into_iter()
                .map(|actor| ProgressCapture {
                    actor,
                    entries: Vec::new(),
                })
                .collect();
            let positions: HashMap<_, _> = agents
                .iter()
                .enumerate()
                .map(|(i, a)| (a.actor.thread_id.to_string(), i))
                .collect();
            for entry in ChangeHistoryStore::open_existing(dir)?.list_entries()? {
                if entry.source_kind.is_authored()
                    && !entry.is_unattributed()
                    && !entry.ephemeral_only
                    && let Some(&i) = positions.get(&entry.thread_id)
                {
                    if !seen.contains(&format!("native:{}", entry.id)) {
                        agents[i].entries.push(entry);
                    }
                }
            }
            agents.retain(|a| !a.entries.is_empty());
            Ok(agents)
        })
        .await?
    }
}

fn delivered_cursors(history: &[ResponseItemEnvelope], context_id: &str) -> Vec<CaptureCursor> {
    let sampled_through = crate::context_manager::sampled_through(history);
    let mut intervals: BTreeMap<&str, Vec<(u64, u64)>> = BTreeMap::new();
    for metadata in history.iter().filter_map(|item| item.metadata.as_ref()) {
        if metadata
            .history_sequence
            .is_none_or(|sequence| sequence > sampled_through)
        {
            continue;
        }
        for receipt in &metadata.capture_deliveries {
            if receipt.context_id != context_id {
                continue;
            }
            for interval in &receipt.intervals {
                if interval.first_chunk <= interval.last_chunk {
                    intervals
                        .entry(&interval.key)
                        .or_default()
                        .push((interval.first_chunk, interval.last_chunk));
                }
            }
        }
    }
    intervals
        .into_iter()
        .filter_map(|(key, mut intervals)| {
            intervals.sort_unstable();
            let mut through_chunk = 0_u64;
            for (first, last) in intervals {
                // A later visible fragment never acknowledges a missing earlier interval.
                if first > through_chunk.saturating_add(1) {
                    break;
                }
                through_chunk = through_chunk.max(last);
            }
            (through_chunk > 0).then(|| CaptureCursor {
                key: key.to_string(),
                through_chunk,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_history::CaptureDeliveryReceipt;
    use codex_history::CodexHarnessMetadata;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ResponseItem;

    fn delivery(
        sequence: u64,
        sampled: u64,
        context: &str,
        first: u64,
        last: u64,
    ) -> ResponseItemEnvelope {
        ResponseItemEnvelope {
            item: ResponseItem::FunctionCallOutput {
                id: None,
                call_id: Some(format!("call-{sequence}")),
                name: None,
                namespace: None,
                output: FunctionCallOutputPayload::from_text("complete evidence".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
            metadata: Some(CodexHarnessMetadata {
                history_sequence: Some(sequence),
                sampled_through_sequence: Some(sampled),
                capture_deliveries: vec![CaptureDeliveryReceipt {
                    operation_id: format!("progress-{sequence}"),
                    context_id: context.to_string(),
                    intervals: vec![CaptureInterval {
                        key: "capture-key".to_string(),
                        change_id: "amber1".to_string(),
                        record_order: 1,
                        legacy_record_key: None,
                        first_chunk: first,
                        last_chunk: last,
                    }],
                }],
                ..Default::default()
            }),
        }
    }

    #[test]
    fn cursors_require_sampled_complete_same_context_evidence() {
        let mut history = vec![delivery(1, 0, "context", 1, 2)];
        assert!(delivered_cursors(&history, "context").is_empty());
        history.push(delivery(2, 1, "context", 3, 4));
        assert_eq!(delivered_cursors(&history, "context")[0].through_chunk, 2);
        assert!(delivered_cursors(&history, "reset").is_empty());
        history[0]
            .metadata
            .as_mut()
            .unwrap()
            .mark_retained_sources_incomplete();
        assert!(delivered_cursors(&history, "context").is_empty());
    }

    #[test]
    fn later_extensions_do_not_acknowledge_gaps() {
        let mut history = vec![
            delivery(1, 3, "context", 1, 2),
            delivery(2, 3, "context", 4, 5),
        ];
        assert_eq!(delivered_cursors(&history, "context")[0].through_chunk, 2);
        history.push(delivery(3, 3, "context", 3, 3));
        assert_eq!(delivered_cursors(&history, "context")[0].through_chunk, 5);
    }

    #[test]
    fn history_budget_truncation_discards_capture_receipts() {
        let mut item = delivery(1, 1, "context", 1, 2);
        if let ResponseItem::FunctionCallOutput { output, .. } = &mut item.item {
            *output = FunctionCallOutputPayload::from_text("evidence ".repeat(100));
        }
        let mut history = crate::context_manager::ContextManager::default();
        history.record_annotated_items(
            std::slice::from_mut(&mut item),
            codex_utils_output_truncation::TruncationPolicy::Tokens(2),
        );
        assert!(delivered_cursors(history.annotated_items(), "context").is_empty());
        // The retained original is independent from the bounded model context.
        assert_eq!(item.metadata.unwrap().capture_deliveries.len(), 1);
    }

    #[tokio::test]
    async fn progress_feature_fact_requires_retained_delivery() {
        use std::sync::{Arc, Mutex};
        let (session, context, _events) =
            crate::session::tests::make_session_and_context_with_rx().await;
        let facts = Arc::new(Mutex::new(Vec::new()));
        assert!(
            session
                .services
                .local_agent_runtime
                .feature_use
                .set(Arc::new({
                    let facts = facts.clone();
                    move |fact| facts.lock().unwrap().push(fact)
                }))
                .is_ok()
        );
        for truncated in [false, true] {
            let mut item = delivery(1, 1, "context", 1, 2);
            let metadata = item.metadata.as_mut().unwrap();
            metadata.capture_deliveries[0].operation_id =
                if truncated { "truncated" } else { "delivered" }.into();
            metadata.history_truncation_token_limit = Some(if truncated { 2 } else { 1000 });
            if let ResponseItem::FunctionCallOutput { output, .. } = &mut item.item {
                *output = FunctionCallOutputPayload::from_text("evidence ".repeat(100));
            }
            session
                .record_annotated_conversation_items(&context, context.model_info(), vec![item])
                .await;
        }
        let facts = facts.lock().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].operation, "delivered");
        assert_eq!(
            facts[0].event,
            codex_protocol::feature_use::FeatureUseEvent::ProgressDelivered
        );
    }

    #[test]
    fn composed_change_source_merges_consecutive_edits_and_preserves_gaps() -> anyhow::Result<()> {
        use codex_change_history::{
            ChangeMetadata, ChangeSourceKind, CoverageStatus, FileSnapshot, PendingFileChange,
        };

        let workspace = tempfile::tempdir()?;
        let store = ChangeHistoryStore::open(workspace.path().join("changes"))?;
        let path = workspace.path().join("notes.txt");
        let record =
            |call_id: &str, before: FileSnapshot, after: FileSnapshot, coverage: CoverageStatus| {
                store.record_change(
                    ChangeMetadata {
                        thread_id: "worker".into(),
                        session_id: "session".into(),
                        turn_id: "turn".into(),
                        call_id: call_id.into(),
                        author: None,
                        source_kind: ChangeSourceKind::ApplyPatch,
                        workspace_root: workspace.path().to_path_buf(),
                        created_at: 1,
                        exit_code: Some(0),
                        summary: None,
                        claim_key: None,
                    },
                    vec![PendingFileChange {
                        rel_path: "notes.txt".into(),
                        abs_path: path.clone(),
                        moved_from: None,
                        before,
                        after,
                        coverage,
                        scope: codex_change_history::ChangeScope::Workspace,
                    }],
                )
            };

        let first = record(
            "c1",
            FileSnapshot::Missing,
            FileSnapshot::Present(b"superseded draft\n".to_vec()),
            CoverageStatus::Complete,
        )?;
        let second = record(
            "c2",
            FileSnapshot::Present(b"superseded draft\n".to_vec()),
            FileSnapshot::Present(b"final text\n".to_vec()),
            CoverageStatus::Complete,
        )?;
        let _skipped = record(
            "c3",
            FileSnapshot::Present(b"final text\n".to_vec()),
            FileSnapshot::Present(b"final text\nskipped edit\n".to_vec()),
            CoverageStatus::Complete,
        )?;
        let after_gap = record(
            "c4",
            FileSnapshot::Present(b"final text\nskipped edit\n".to_vec()),
            FileSnapshot::Present(b"final text\nskipped edit\nafter gap\n".to_vec()),
            CoverageStatus::Partial {
                reason: "size cap".into(),
            },
        )?;

        // Consecutive edits compose into one cumulative diff without intermediate text.
        let consecutive =
            render_composed_change_source(&store, &[first.clone(), second.clone()], 16_000)?
                .expect("within budget");
        assert!(consecutive.contains("+final text"), "{consecutive}");
        assert!(!consecutive.contains("superseded draft"), "{consecutive}");
        assert!(
            !consecutive.contains("Incomplete capture coverage."),
            "{consecutive}"
        );

        // Consecutive edits before a gap compose, while the skipped edit is not folded in.
        let with_gap = render_composed_change_source(
            &store,
            &[first.clone(), second.clone(), after_gap.clone()],
            16_000,
        )?
        .expect("within budget");
        assert!(
            with_gap.contains("Incomplete capture coverage.\n"),
            "{with_gap}"
        );
        assert!(with_gap.contains("+final text"), "{with_gap}");
        assert!(with_gap.contains("+after gap"), "{with_gap}");
        assert!(!with_gap.contains("superseded draft"), "{with_gap}");
        assert!(!with_gap.contains("+skipped edit"), "{with_gap}");
        assert!(with_gap.contains("# coverage:"), "{with_gap}");

        // Per-record final handoff rendering keeps coverage diagnostics and budget enforcement.
        let single = render_change_record(&store, &after_gap, 16_000)?.expect("within budget");
        assert!(
            single.contains("Incomplete capture coverage.\n"),
            "{single}"
        );
        assert!(single.contains("+after gap"), "{single}");
        assert!(render_change_record(&store, &after_gap, 10)?.is_none());

        // Output budget is enforced for composed reads too.
        assert!(render_composed_change_source(&store, &[first, second], 10)?.is_none());
        Ok(())
    }
}
