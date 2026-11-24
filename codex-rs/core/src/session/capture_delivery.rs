//! Native retained records are immutable. A sampled, complete history item is
//! the receipt for a change, not the worker's return or an unprinted result.

use super::Session;
use crate::agent_communication::FinalCaptureAttachment;
use codex_change_history::{ChangeHistoryStore, ChangeId, compose_diff, expand_id_spec};
use codex_history::ResponseItemEnvelope;
use codex_protocol::agent_capture::*;
use codex_protocol::{SessionId, ThreadId};
use std::collections::{BTreeMap, HashMap, HashSet};

fn interval(id: &ChangeId, sequence: u64) -> CaptureInterval {
    CaptureInterval {
        key: format!("native:{id}"),
        change_id: id.to_string(),
        record_order: sequence,
        legacy_record_key: None,
        first_chunk: 1,
        last_chunk: 1,
    }
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
                            if entry.thread_id != recipient && entry.source_kind.is_authored() {
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
        let rendered = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let store = ChangeHistoryStore::open_existing(dir)?;
            let records = store.load_records(&ids?)?;
            let mut body = String::new();
            if records
                .iter()
                .flat_map(|r| &r.files)
                .any(|f| !f.coverage.is_complete())
            {
                body.push_str("Incomplete capture coverage.\n");
            }
            for diff in compose_diff(&store, &records, &[])? {
                body.push_str(&diff.unified_diff);
            }
            Ok(body)
        })
        .await;
        match rendered {
            Ok(Ok(body)) if body.len() + selection.len() <= 16_000 => {
                (format!("mchanges:{selection}\n{body}"), intervals)
            }
            Ok(Ok(_)) => (
                format!(
                    "Captured changes exceed the attachment budget. Read mchanges:{selection}."
                ),
                Vec::new(),
            ),
            Ok(Err(error)) => (
                format!("mchanges:{selection}: CONTENT NOT ATTACHED: {error}"),
                Vec::new(),
            ),
            Err(error) => (
                format!("mchanges:{selection}: CONTENT NOT ATTACHED: {error}"),
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
    ) -> anyhow::Result<Vec<FrozenAgentCapture>> {
        let (_, seen) = self.capture_context().await;
        let dir = crate::exec_env::change_history_dir(self.session_id());
        tokio::task::spawn_blocking(move || {
            if !dir.try_exists()? {
                return Ok(Vec::new());
            }
            let seen: HashSet<_> = seen.into_iter().map(|cursor| cursor.key).collect();
            let mut agents: Vec<_> = selected
                .into_iter()
                .map(|actor| FrozenAgentCapture {
                    actor,
                    intervals: Vec::new(),
                })
                .collect();
            let positions: HashMap<_, _> = agents
                .iter()
                .enumerate()
                .map(|(i, a)| (a.actor.thread_id.to_string(), i))
                .collect();
            for entry in ChangeHistoryStore::open_existing(dir)?.list_entries()? {
                if entry.source_kind.is_authored()
                    && let Some(&i) = positions.get(&entry.thread_id)
                {
                    let interval = interval(&entry.id, entry.sequence);
                    if !seen.contains(&interval.key) {
                        agents[i].intervals.push(interval);
                    }
                }
            }
            agents.retain(|a| !a.intervals.is_empty());
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
}
