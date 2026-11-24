//! Retained captures stay with the original resource host, not UI subscribers.

use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessageSender;
use codex_app_server_protocol as wire;
use codex_core::AgentHostResources;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::sync::Weak;
use tokio_util::sync::CancellationToken;

pub(crate) fn host_resources(
    outgoing: &Arc<OutgoingMessageSender>,
    connection: ConnectionId,
    disconnect: CancellationToken,
    tree_shutdown: CancellationToken,
    observe_before_tool: bool,
) -> AgentHostResources {
    let host = HostRequests {
        outgoing: Arc::downgrade(outgoing),
        connection,
        disconnect,
        tree_shutdown,
    };
    let freezing = host.clone();
    let progress = host.clone();
    let capture = host.clone();
    let admission = host.clone();
    let foreign = host.clone();
    let before_tool_host = host.clone();
    AgentHostResources {
        route_foreign: Some(Arc::new(move |request, cancellation| {
            let host = foreign.clone();
            Box::pin(async move {
                let caller = request.source.actor_id;
                host.request(
                    wire::ServerRequestPayload::AgentForeignRoute(request),
                    caller,
                    cancellation,
                )
                .await
            })
        })),
        prepare_child: Some(Arc::new(move |request, cancellation| {
            let host = admission.clone();
            Box::pin(async move {
                let caller = request.parent_thread_id;
                host.request(
                    wire::ServerRequestPayload::AgentPrepareChildHost(request),
                    caller,
                    cancellation,
                )
                .await
            })
        })),
        before_tool: if observe_before_tool {
            Some(Arc::new(move |request, cancellation| {
                let host = before_tool_host.clone();
                Box::pin(async move {
                    let thread = ThreadId::from_string(&request.thread_id).map_err(|err| {
                        CodexErr::InvalidRequest(format!("invalid thread_id: {err}"))
                    })?;
                    host.request(
                        wire::ServerRequestPayload::ExecutionBeforeTool(request),
                        thread,
                        cancellation,
                    )
                    .await
                })
            }))
        } else {
            None
        },
        record_feature_use: None,
        freeze_progress: Arc::new(move |request, cancellation| {
            let host = freezing.clone();
            Box::pin(async move {
                let caller = request.caller_thread_id;
                host.request(
                    wire::ServerRequestPayload::AgentFreezeProgress(request),
                    caller,
                    cancellation,
                )
                .await
            })
        }),
        prepare_progress_output: Arc::new(move |request, cancellation| {
            let host = progress.clone();
            Box::pin(async move {
                let caller = request.caller_thread_id;
                host.request(
                    wire::ServerRequestPayload::AgentPrepareProgressOutput(request),
                    caller,
                    cancellation,
                )
                .await
            })
        }),
        prepare_final_capture: Arc::new(move |request, cancellation| {
            let host = capture.clone();
            Box::pin(async move {
                let caller = request.thread_id;
                host.request(
                    wire::ServerRequestPayload::AgentPrepareFinalCapture(request),
                    caller,
                    cancellation,
                )
                .await
            })
        }),
        project_final_capture: Arc::new(move |request, cancellation| {
            let host = host.clone();
            Box::pin(async move {
                let caller = request.thread_id;
                host.request(
                    wire::ServerRequestPayload::AgentProjectFinalCapture(request),
                    caller,
                    cancellation,
                )
                .await
            })
        }),
    }
}

#[derive(Clone)]
struct HostRequests {
    outgoing: Weak<OutgoingMessageSender>,
    connection: ConnectionId,
    disconnect: CancellationToken,
    tree_shutdown: CancellationToken,
}

impl HostRequests {
    async fn request<T: DeserializeOwned>(
        self,
        payload: wire::ServerRequestPayload,
        thread: ThreadId,
        cancellation: CancellationToken,
    ) -> Result<T> {
        let outgoing = self
            .outgoing
            .upgrade()
            .ok_or_else(|| CodexErr::Fatal("capture resource host is unavailable".into()))?;
        let shutdown = outgoing.host_request_shutdown();
        let (mut completed, result) = tokio::sync::oneshot::channel();
        let requests = Arc::clone(&outgoing);
        // A dropped tool future still retires its exact request. The outgoing owner
        // stops admission, cancels and joins these cleanup tasks at shutdown.
        outgoing.track_host_request(async move {
            if cancellation.is_cancelled() || self.disconnect.is_cancelled()
                || self.tree_shutdown.is_cancelled() || shutdown.is_cancelled()
                || completed.is_closed()
            {
                let _ = completed.send(Err(CodexErr::TurnAborted));
                return;
            }
            // Allocate before the send, so cancellation under transport backpressure
            // can retire the registered callback as well as the response waiter.
            let id = requests.next_request_id();
            let operation = async {
                let (_, response) = requests.send_request_with_lifetime(
                    id.clone(), Some(&[self.connection]), payload, Some(thread), false,
                ).await;
                response.await
                    .map_err(|_| CodexErr::Fatal("capture resource request was canceled".into()))?
                    .map_err(|error| CodexErr::Fatal(format!("capture resource host: {}", error.message)))
            };
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(CodexErr::TurnAborted),
                _ = self.tree_shutdown.cancelled() => Err(CodexErr::TurnAborted),
                _ = shutdown.cancelled() => Err(CodexErr::TurnAborted),
                _ = self.disconnect.cancelled() => Err(CodexErr::Fatal("capture resource host disconnected".into())),
                _ = completed.closed() => Err(CodexErr::TurnAborted),
                response = operation => response,
            };
            if requests.cancel_request(&id).await && !self.disconnect.is_cancelled() {
                let notification = wire::ServerNotification::ServerRequestResolved(
                    wire::ServerRequestResolvedNotification {
                        thread_id: thread.to_string(), request_id: id,
                    },
                );
                let connections = [self.connection];
                tokio::select! {
                    _ = shutdown.cancelled() => {},
                    _ = self.disconnect.cancelled() => {},
                    _ = requests.send_server_notification_to_connections(&connections, notification) => {},
                }
            }
            let _ = completed.send(result);
        });
        let response = result
            .await
            .map_err(|_| CodexErr::Fatal("capture resource cleanup stopped".into()))??;
        serde_json::from_value(response)
            .map_err(|error| CodexErr::Fatal(format!("invalid capture resource response: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outgoing_message::OutgoingEnvelope;
    use crate::outgoing_message::OutgoingMessage;
    use codex_protocol::agent_capture::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn request() -> FinalCaptureRequest {
        FinalCaptureRequest {
            root_thread_id: ThreadId::new(),
            thread_id: ThreadId::new(),
            operation_id: "native-call".into(),
            recipient_thread_id: ThreadId::new(),
            recipient_cwd: Some("/recipient".into()),
            sources: vec!["mchanges:amber1".into()],
        }
    }

    fn sender(capacity: usize) -> (Arc<OutgoingMessageSender>, mpsc::Receiver<OutgoingEnvelope>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Arc::new(OutgoingMessageSender::new(
                tx,
                codex_analytics::AnalyticsEventsClient::disabled(),
            )),
            rx,
        )
    }

    async fn request_id(
        rx: &mut mpsc::Receiver<OutgoingEnvelope>,
        owner: ConnectionId,
    ) -> wire::RequestId {
        let Some(OutgoingEnvelope::ToConnection {
            connection_id,
            message: OutgoingMessage::Request(request),
            ..
        }) = rx.recv().await
        else {
            panic!("expected resource request");
        };
        assert_eq!(connection_id, owner);
        request.id().clone()
    }

    #[tokio::test]
    async fn capture_requests_are_owner_only_and_never_replayed() {
        let (outgoing, mut rx) = sender(4);
        let owner = ConnectionId(7);
        let callbacks = host_resources(
            &outgoing,
            owner,
            CancellationToken::new(),
            CancellationToken::new(),
            false,
        );
        let input = request();
        let thread = input.thread_id;
        let task = tokio::spawn((callbacks.prepare_final_capture)(
            input,
            CancellationToken::new(),
        ));
        let id = request_id(&mut rx, owner).await;
        assert!(
            outgoing
                .pending_requests_for_thread(thread)
                .await
                .is_empty()
        );
        outgoing
            .replay_requests_to_connection_for_thread(ConnectionId(8), thread)
            .await;
        assert!(rx.try_recv().is_err());
        outgoing
            .notify_client_response(ConnectionId(8), id.clone(), json!({"capture": null}))
            .await;
        assert!(!task.is_finished());
        outgoing
            .notify_client_response(
                owner,
                id,
                json!({"capture": {"resourceRef": "retained:1", "fullText": "changes"}}),
            )
            .await;
        let response = task.await.unwrap().unwrap();
        assert_eq!(response.capture.unwrap().resource_ref, "retained:1");
        outgoing.shutdown_host_requests().await;
    }

    #[tokio::test]
    async fn canceled_and_abandoned_requests_retire_exact_host_acquisition() {
        for abandon in [false, true] {
            let (outgoing, mut rx) = sender(4);
            let owner = ConnectionId(7);
            let callbacks = host_resources(
                &outgoing,
                owner,
                CancellationToken::new(),
                CancellationToken::new(),
                false,
            );
            let cancellation = CancellationToken::new();
            let task = tokio::spawn((callbacks.prepare_final_capture)(
                request(),
                cancellation.clone(),
            ));
            let id = request_id(&mut rx, owner).await;
            if abandon {
                task.abort();
            } else {
                cancellation.cancel();
            }
            let Some(OutgoingEnvelope::ToConnection {
                connection_id,
                message: OutgoingMessage::AppServerNotification(envelope),
                ..
            }) = rx.recv().await
            else {
                panic!("expected cancellation notification");
            };
            assert_eq!(connection_id, owner);
            let wire::ServerNotification::ServerRequestResolved(resolved) = envelope.notification
            else {
                panic!("wrong notification")
            };
            assert_eq!(resolved.request_id, id);
            assert!(!outgoing.cancel_request(&id).await);
            let result = task.await;
            if abandon {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_err());
            }
            outgoing.shutdown_host_requests().await;
        }
    }

    #[tokio::test]
    async fn disconnect_and_native_tree_shutdown_settle_requests() {
        for disconnect_host in [false, true] {
            let (outgoing, mut rx) = sender(4);
            let owner = ConnectionId(7);
            let disconnect = CancellationToken::new();
            let tree = CancellationToken::new();
            let callbacks =
                host_resources(&outgoing, owner, disconnect.clone(), tree.clone(), false);
            let task = tokio::spawn((callbacks.prepare_final_capture)(
                request(),
                CancellationToken::new(),
            ));
            let id = request_id(&mut rx, owner).await;
            if disconnect_host {
                disconnect.cancel();
            } else {
                tree.cancel();
            }
            assert!(task.await.unwrap().is_err());
            assert!(!outgoing.cancel_request(&id).await);
            outgoing.shutdown_host_requests().await;
        }
    }

    #[tokio::test]
    async fn before_tool_callback_is_opt_in_and_owner_only() {
        let (outgoing, mut rx) = sender(4);
        let owner = ConnectionId(7);
        let unobserved = host_resources(
            &outgoing,
            owner,
            CancellationToken::new(),
            CancellationToken::new(),
            false,
        );
        assert!(unobserved.before_tool.is_none());

        let observed = host_resources(
            &outgoing,
            owner,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        );
        let before_tool = observed.before_tool.expect("before_tool callback");
        let thread = ThreadId::new();
        let task = tokio::spawn(before_tool(
            wire::ExecutionBeforeToolParams {
                thread_id: thread.to_string(),
                turn_id: "turn-1".into(),
                item_id: "item-1".into(),
                tool: "Bash".into(),
                cwd: "/tmp".into(),
                input: "echo hi".into(),
                session_tmp: None,
            },
            CancellationToken::new(),
        ));
        let id = request_id(&mut rx, owner).await;
        outgoing.notify_client_response(owner, id, json!({})).await;
        let _ = task.await.unwrap().unwrap();
        outgoing.shutdown_host_requests().await;
    }
}
