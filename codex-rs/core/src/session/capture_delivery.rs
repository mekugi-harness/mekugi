//! Capture selections borrow the host's retained evidence. Delivery is proven by
//! native history, not by a callback reply or an unprinted JavaScript result.

use std::collections::BTreeMap;

use codex_history::ResponseItemEnvelope;
use codex_protocol::agent_capture::*;
use tokio_util::sync::CancellationToken;

use super::Session;

impl Session {
    pub(crate) async fn project_final_capture(
        &self,
        capture: &crate::agent_communication::FinalCaptureAttachment,
    ) -> String {
        if capture.resource_ref.is_empty() {
            return capture.full_text.clone();
        }
        let project = async {
            let resources = self
                .services
                .local_agent_runtime
                .host_resources
                .get()
                .ok_or_else(|| "capture resource owner unavailable".to_string())?;
            let (recipient_context_id, seen) = self.capture_context().await;
            let request = FinalCaptureProjectRequest {
                root_thread_id: capture.root_thread_id,
                thread_id: capture.sender_thread_id,
                operation_id: capture.operation_id.clone(),
                recipient_thread_id: self.thread_id,
                recipient_cwd: capture.recipient_cwd.clone(),
                recipient_context_id,
                resource_ref: capture.resource_ref.clone(),
                seen,
            };
            let cancellation = self.services.local_agent_runtime.shutdown.child_token();
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err("capture projection cancelled".to_string()),
                result = (resources.project_final_capture)(request, cancellation.clone()) =>
                    result.map(|response| response.text).map_err(|error| error.to_string()),
            }
        }
        .await;
        match project {
            Ok(text) => text,
            Err(error) => format!(
                "{}\nCapture projection unavailable: {error}. Full frozen selection retained above.\n",
                capture.full_text
            ),
        }
    }

    pub(crate) async fn capture_context(&self) -> (String, Vec<CaptureCursor>) {
        let state = self.state.lock().await;
        let context_id = self.current_window_from_state(&state).2.to_string();
        let seen = delivered_cursors(state.history.annotated_items(), &context_id);
        (context_id, seen)
    }

    pub(crate) async fn select_progress_ranges(
        &self,
        operation_id: &str,
        selected: Vec<ProgressActor>,
        cancellation: CancellationToken,
    ) -> anyhow::Result<Option<ProgressFreezeResponse>> {
        let Some(resources) = self.services.local_agent_runtime.host_resources.get() else {
            return Ok(None);
        };
        let (recipient_context_id, seen) = self.capture_context().await;
        let request = ProgressFreezeRequest {
            root_thread_id: self.session_id().into(),
            operation_id: operation_id.to_string(),
            caller_thread_id: self.thread_id,
            recipient_context_id,
            selected,
            seen,
        };
        let frozen = tokio::select! {
            biased;
            _ = cancellation.cancelled() => anyhow::bail!("capture selection cancelled"),
            result = (resources.freeze_progress)(request, cancellation.clone()) => result?,
        };
        Ok((!frozen.agents.is_empty()).then_some(frozen))
    }

    pub(crate) async fn prepare_progress_output(
        &self,
        operation_id: &str,
        frozen: ProgressFreezeResponse,
        agents: Vec<PreparedProgressAgent>,
        cancellation: CancellationToken,
    ) -> anyhow::Result<ProgressOutputResponse> {
        let resources = self
            .services
            .local_agent_runtime
            .host_resources
            .get()
            .ok_or_else(|| anyhow::anyhow!("capture resource owner unavailable"))?;
        let (recipient_context_id, _) = self.capture_context().await;
        let request = ProgressOutputRequest {
            root_thread_id: self.session_id().into(),
            operation_id: operation_id.to_string(),
            caller_thread_id: self.thread_id,
            recipient_context_id,
            selection_ref: frozen.selection_ref,
            agents,
            coverage: frozen.coverage,
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => anyhow::bail!("capture output cancelled"),
            result = (resources.prepare_progress_output)(request, cancellation.clone()) => Ok(result?),
        }
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
        let mut resources =
            crate::agent::host_resources::tests::admission_resources(Arc::new(|_, _| {
                Box::pin(async { panic!("not spawning") })
            }));
        resources.record_feature_use = Some(Arc::new({
            let facts = facts.clone();
            move |fact| facts.lock().unwrap().push(fact)
        }));
        assert!(
            session
                .services
                .local_agent_runtime
                .host_resources
                .set(resources)
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

    #[tokio::test]
    async fn final_mail_projects_when_consumed_and_reset_restores_eligibility() {
        use crate::agent_communication::{
            AgentCommunicationContext, AgentCommunicationKind, FinalCaptureAttachment,
            MessageDelivery,
        };
        use codex_protocol::protocol::InterAgentCommunication;
        use codex_protocol::{AgentPath, ThreadId};
        use std::sync::{Arc, Mutex};
        let (session, context, _events) =
            crate::session::tests::make_session_and_context_with_rx().await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let resources = crate::AgentHostResources {
            route_foreign: None,
            prepare_child: None,
            record_feature_use: None,
            freeze_progress: Arc::new(|_, _| Box::pin(async { panic!("not progress") })),
            prepare_progress_output: Arc::new(|_, _| Box::pin(async { panic!("not progress") })),
            prepare_final_capture: Arc::new(|_, _| Box::pin(async { panic!("already frozen") })),
            project_final_capture: Arc::new({
                let requests = Arc::clone(&requests);
                move |request, _| {
                    let seen = !request.seen.is_empty();
                    requests.lock().unwrap().push(request);
                    Box::pin(async move {
                        Ok(FinalCaptureProjectResponse {
                            text: if seen {
                                "unseen extension"
                            } else {
                                "full frozen evidence"
                            }
                            .into(),
                        })
                    })
                }
            }),
        };
        assert!(
            session
                .services
                .local_agent_runtime
                .host_resources
                .set(resources)
                .is_ok()
        );
        let (context_id, _) = session.capture_context().await;
        session.state.lock().await.history.record_annotated_items(
            &mut [delivery(1, 1, &context_id, 1, 2)],
            codex_utils_output_truncation::TruncationPolicy::Tokens(1000),
        );
        let sender = ThreadId::new();
        let mut publication =
            AgentCommunicationContext::new(AgentCommunicationKind::Result, sender);
        publication.result_operation = Some("final".into());
        publication.final_capture = Some(Arc::new(FinalCaptureAttachment {
            root_thread_id: session.session_id().into(),
            sender_thread_id: sender,
            operation_id: "final".into(),
            recipient_cwd: Some("/workspace".into()),
            resource_ref: "frozen".into(),
            full_text: "full frozen evidence".into(),
        }));
        for reset in [false, true] {
            let delivery = MessageDelivery::publish_with_context(&publication, &[]);
            if reset {
                session.replace_history(Vec::new(), None).await;
            }
            let mail = crate::PendingAgentMessage::new(
                InterAgentCommunication::new(
                    AgentPath::try_from("/root/child").unwrap(),
                    AgentPath::root(),
                    Vec::new(),
                    "original final answer".into(),
                    false,
                ),
                Some(delivery.clone()),
            );
            assert_eq!(mail.result_sender(), Some(sender));
            session
                .record_inter_agent_communication(&context, context.model_info(), mail)
                .await;
            assert!(delivery.final_capture().is_none());
            let history = session.clone_history().await;
            let text = serde_json::to_string(&history.raw_items().collect::<Vec<_>>()).unwrap();
            assert!(text.contains("original final answer"));
            assert!(text.contains(if reset {
                "full frozen evidence"
            } else {
                "unseen extension"
            }));
        }
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].seen[0].through_chunk, 2);
        assert!(requests[1].seen.is_empty());
        assert!(
            requests
                .iter()
                .all(|request| request.operation_id == "final" && request.resource_ref == "frozen")
        );
    }
}
