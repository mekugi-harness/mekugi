use super::*;
use crate::agent_communication::AgentCommunicationContext;
use crate::agent_communication::AgentCommunicationKind;
use crate::agent_communication::MessageDelivery;
use crate::thread_input::pending_wait_activity;
use pretty_assertions::assert_eq;

async fn fixture() -> (
    ThreadManager,
    Arc<crate::session::session::Session>,
    Arc<TurnContext>,
    Vec<ThreadId>,
) {
    let (manager, session, turn, children, _events) = fixture_with_events().await;
    (manager, session, turn, children)
}

async fn fixture_with_events() -> (
    ThreadManager,
    Arc<crate::session::session::Session>,
    Arc<TurnContext>,
    Vec<ThreadId>,
    async_channel::Receiver<codex_protocol::protocol::Event>,
) {
    let (session, turn, events) = crate::session::tests::make_session_and_context_with_rx().await;
    let mut session = Arc::try_unwrap(session).ok().expect("unique test session");
    let mut turn = Arc::try_unwrap(turn).ok().expect("unique test turn");
    let manager = thread_manager();
    let mut config = (*turn.config).clone();
    config.features.enable(Feature::MultiAgentV2).unwrap();
    set_turn_config(&mut turn, config.clone());
    let root = manager
        .start_thread(StartThreadOptions::new(config))
        .await
        .unwrap();
    set_agent_control(
        &mut session,
        root.thread
            .session
            .services
            .local_agent_runtime
            .control(root.thread.session.session_id()),
    );
    session.thread_id = root.thread_id;
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let mut children = Vec::new();
    for name in ["first", "second"] {
        SpawnAgentHandlerV2::default()
            .handle(invocation(
                session.clone(),
                turn.clone(),
                "spawn_agent",
                function_payload(json!({"task_name":name,"message":"work"})),
            ))
            .await
            .unwrap();
        children.push(
            session
                .services
                .local_agent_runtime
                .resolve_agent_reference(
                    session.thread_id,
                    &turn.session_source,
                    &format!("/root/{name}"),
                )
                .await
                .unwrap(),
        );
    }
    (manager, session, turn, children, events)
}

async fn mail(session: &crate::session::session::Session, sender: ThreadId, result: bool) {
    let metadata = session
        .services
        .local_agent_runtime
        .ensure_agent_known(sender)
        .unwrap();
    let context = AgentCommunicationContext::new(
        if result {
            AgentCommunicationKind::Result
        } else {
            AgentCommunicationKind::Message
        },
        sender,
    );
    let delivery = MessageDelivery::publish_with_context(&context, &[]);
    session
        .input_queue
        .enqueue_mailbox_communication(
            crate::PendingAgentMessage::new(
                InterAgentCommunication::new(
                    metadata.agent_path.unwrap(),
                    AgentPath::root(),
                    Vec::new(),
                    "content".into(),
                    false,
                ),
                Some(delivery),
            ),
            Default::default(),
        )
        .await;
}

#[tokio::test]
async fn all_wait_collects_every_selected_result_without_consuming_mail() {
    let (_manager, session, turn, children) = fixture().await;
    let handler = WaitAgentHandlerV2::default();
    let mut pending = Box::pin(handler.handle(invocation(
        session.clone(),
        turn.clone(),
        "wait_agent",
        function_payload(json!({"wait_mode":"all"})),
    )));
    assert!(futures::poll!(&mut pending).is_pending());
    mail(&session, children[1], true).await;
    assert!(futures::poll!(&mut pending).is_pending());
    mail(&session, children[0], true).await;
    let output = pending
        .await
        .unwrap()
        .code_mode_result(&function_payload(json!({})));
    assert_eq!(output["completed_agents"].as_array().unwrap().len(), 2);
    assert_eq!(
        pending_wait_activity(&session, &turn.sub_id)
            .await
            .results
            .len(),
        2
    );
}

#[tokio::test]
async fn any_wait_returns_first_result_and_all_wait_addressed_mail_supersedes() {
    for mode in ["any", "all"] {
        let (_manager, session, turn, children) = fixture().await;
        let handler = WaitAgentHandlerV2::default();
        let mut pending = Box::pin(handler.handle(invocation(
            session.clone(),
            turn.clone(),
            "wait_agent",
            function_payload(json!({"wait_mode":mode})),
        )));
        assert!(futures::poll!(&mut pending).is_pending());
        mail(&session, children[0], true).await;
        if mode == "all" {
            assert!(futures::poll!(&mut pending).is_pending());
            mail(&session, children[1], false).await;
        }
        let output = pending
            .await
            .unwrap()
            .code_mode_result(&function_payload(json!({})));
        assert_eq!(output["message"], "Wait completed.");
    }
}

#[tokio::test]
async fn selected_child_interruption_settles_all_slot() {
    let (manager, session, turn, children) = fixture().await;
    let handler = WaitAgentHandlerV2::default();
    let mut pending = Box::pin(handler.handle(invocation(
        session.clone(),
        turn.clone(),
        "wait_agent",
        function_payload(json!({"wait_mode":"all"})),
    )));
    assert!(futures::poll!(&mut pending).is_pending());
    mail(&session, children[0], true).await;
    let child = manager.get_thread(children[1]).await.unwrap();
    let child_turn = child.session.new_default_turn().await;
    child
        .session
        .send_event(
            &child_turn,
            EventMsg::TurnAborted(TurnAbortedEvent {
                root_turn_id: None,
                turn_id: Some(child_turn.sub_id.clone()),
                started_at: None,
                reason: TurnAbortReason::Interrupted,
                error: None,
                completed_at: None,
                duration_ms: None,
            }),
        )
        .await;
    let output = pending
        .await
        .unwrap()
        .code_mode_result(&function_payload(json!({})));
    assert_eq!(output["completed_agents"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn omitted_yield_is_indefinite_and_caller_cancellation_preserves_mail() {
    let (_manager, session, turn, children, events) = fixture_with_events().await;
    let handler = WaitAgentHandlerV2::default();
    let call = invocation(
        session.clone(),
        turn.clone(),
        "wait_agent",
        function_payload(json!({"wait_mode":"all"})),
    );
    let cancellation = call.cancellation_token.clone();
    let mut pending = Box::pin(handler.handle(call));
    assert!(futures::poll!(&mut pending).is_pending());
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3600)).await;
    assert!(futures::poll!(&mut pending).is_pending());
    tokio::time::resume();
    mail(&session, children[0], true).await;
    assert!(futures::poll!(&mut pending).is_pending());
    cancellation.cancel();
    let output = pending
        .await
        .unwrap()
        .code_mode_result(&function_payload(json!({})));
    assert_eq!(output["message"], "Wait interrupted by new input.");
    assert_wait_item_settled(
        &events,
        codex_protocol::items::CollabAgentToolCallStatus::Interrupted,
    );
    assert!(
        pending_wait_activity(&session, &turn.sub_id)
            .await
            .results
            .contains(&children[0])
    );
}

#[tokio::test]
async fn wait_rejects_retired_fields_invalid_mode_and_short_progress_interval() {
    let (session, turn) = make_session_and_context().await;
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    for args in [
        json!({"timeout_ms":1}),
        json!({"compact":true}),
        json!({"reload":{"sources":[]}}),
        json!({"wait_mode":"both"}),
        json!({"yield_time_ms":299999}),
    ] {
        assert!(
            WaitAgentHandlerV2::default()
                .handle(invocation(
                    session.clone(),
                    turn.clone(),
                    "wait_agent",
                    function_payload(args)
                ))
                .await
                .is_err()
        );
    }
}

fn progress_resources(
    freeze_progress: crate::CaptureResourceCallback<
        codex_protocol::agent_capture::ProgressFreezeRequest,
        codex_protocol::agent_capture::ProgressFreezeResponse,
    >,
) -> crate::AgentHostResources {
    crate::AgentHostResources {
        route_foreign: None,
        prepare_child: None,
        record_feature_use: None,
        freeze_progress,
        prepare_progress_output: Arc::new(|_, _| {
            Box::pin(async { panic!("unchanged or cancelled selection must not prepare progress") })
        }),
        prepare_final_capture: Arc::new(|_, _| {
            Box::pin(async { panic!("progress must not prepare final capture") })
        }),
        project_final_capture: Arc::new(|_, _| {
            Box::pin(async { panic!("progress must not project final capture") })
        }),
    }
}

#[tokio::test]
async fn unchanged_progress_intervals_do_not_wake_and_keep_the_native_wait_pending() {
    use codex_protocol::agent_capture::{CaptureCoverage, ProgressFreezeResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (manager, session, turn, _) = fixture().await;
    let probes = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let mut resources = progress_resources(Arc::new({
        let probes = probes.clone();
        let started = started.clone();
        move |request, _| {
            assert_eq!(request.selected.len(), 2);
            assert!(
                request
                    .selected
                    .iter()
                    .all(|actor| actor.task_name.starts_with("/root/"))
            );
            probes.fetch_add(1, Ordering::SeqCst);
            started.notify_one();
            Box::pin(async {
                Ok(ProgressFreezeResponse {
                    selection_ref: String::new(),
                    publication_revision: 0,
                    agents: Vec::new(),
                    coverage: CaptureCoverage {
                        status: "complete".into(),
                        diagnostics: Vec::new(),
                    },
                })
            })
        }
    }));
    resources.record_feature_use = Some(Arc::new({
        let started = started.clone();
        move |fact| {
            if matches!(
                fact.event,
                codex_protocol::feature_use::FeatureUseEvent::WaitStarted { .. }
            ) {
                started.notify_one();
            }
        }
    }));
    assert!(
        manager
            .get_thread(session.thread_id)
            .await
            .unwrap()
            .install_agent_host_resources(resources)
    );
    let handler = WaitAgentHandlerV2::default();
    let call = invocation(
        session,
        turn,
        "wait_agent",
        function_payload(json!({"yield_time_ms":300000})),
    );
    let cancellation = call.cancellation_token.clone();
    tokio::time::pause();
    let mut pending = Box::pin(handler.handle(call));
    assert!(futures::poll!(&mut pending).is_pending());
    tokio::select! {
        _ = &mut pending => panic!("wait completed before progress collection"),
        _ = started.notified() => {}
    }
    for expected in 1..=2 {
        tokio::time::advance(Duration::from_millis(300000)).await;
        assert!(futures::poll!(&mut pending).is_pending());
        tokio::select! {
            _ = &mut pending => panic!("unchanged progress woke the wait"),
            _ = started.notified() => {}
        }
        assert_eq!(probes.load(Ordering::SeqCst), expected);
    }
    cancellation.cancel();
    let output = pending
        .await
        .unwrap()
        .code_mode_result(&function_payload(json!({})));
    tokio::time::resume();
    assert_eq!(output["progress"], "");
}

#[tokio::test]
async fn native_mail_abandons_inflight_progress_selection_and_cancels_its_request() {
    let (manager, session, turn, children) = fixture().await;
    let observed = Arc::new(std::sync::Mutex::new(None));
    let started = Arc::new(tokio::sync::Notify::new());
    let mut resources = progress_resources(Arc::new({
        let observed = observed.clone();
        let started = started.clone();
        move |_, cancellation| {
            *observed.lock().unwrap() = Some(cancellation);
            started.notify_one();
            Box::pin(std::future::pending())
        }
    }));
    resources.record_feature_use = Some(Arc::new({
        let started = started.clone();
        move |fact| {
            if matches!(
                fact.event,
                codex_protocol::feature_use::FeatureUseEvent::WaitStarted { .. }
            ) {
                started.notify_one();
            }
        }
    }));
    assert!(
        manager
            .get_thread(session.thread_id)
            .await
            .unwrap()
            .install_agent_host_resources(resources)
    );
    let handler = WaitAgentHandlerV2::default();
    tokio::time::pause();
    let mut pending = Box::pin(handler.handle(invocation(
        session.clone(),
        turn,
        "wait_agent",
        function_payload(json!({"wait_mode":"all","yield_time_ms":300000})),
    )));
    assert!(futures::poll!(&mut pending).is_pending());
    tokio::select! {
        _ = &mut pending => panic!("wait completed before progress collection"),
        _ = started.notified() => {}
    }
    tokio::time::advance(Duration::from_millis(300000)).await;
    assert!(futures::poll!(&mut pending).is_pending());
    tokio::select! {
        _ = &mut pending => panic!("wait completed during progress selection"),
        _ = started.notified() => {}
    }
    let request = observed
        .lock()
        .unwrap()
        .clone()
        .expect("freeze request started");
    assert!(!request.is_cancelled());
    mail(&session, children[0], false).await;
    let output = pending
        .await
        .unwrap()
        .code_mode_result(&function_payload(json!({})));
    tokio::time::resume();
    assert_eq!(output["message"], "Wait completed.");
    assert!(request.is_cancelled());
    assert_eq!(output["progress"], "");
}

#[tokio::test]
async fn wait_notifications_identify_selected_agents() {
    let (_manager, session, turn, children, events) = fixture_with_events().await;
    for child in &children {
        mail(&session, *child, true).await;
    }
    WaitAgentHandlerV2::default()
        .handle(invocation(
            session,
            turn,
            "wait_agent",
            function_payload(json!({"wait_mode":"all"})),
        ))
        .await
        .unwrap();
    let mut observations = 0;
    while let Ok(event) = events.try_recv() {
        let item = match event.msg {
            EventMsg::ItemStarted(event) => event.item,
            EventMsg::ItemCompleted(event) => event.item,
            _ => continue,
        };
        if let codex_protocol::items::TurnItem::CollabAgentToolCall(item) = item
            && item.tool == codex_protocol::items::CollabAgentTool::Wait
        {
            assert_eq!(item.receiver_thread_ids.len(), children.len());
            assert!(
                children
                    .iter()
                    .all(|id| item.receiver_thread_ids.contains(id))
            );
            assert_eq!(item.receiver_agents.len(), children.len());
            observations += 1;
        }
    }
    assert_eq!(observations, 2);
}

#[tokio::test]
async fn wait_finishes_its_item_on_progress_failure() {
    let (manager, session, turn, _, events) = fixture_with_events().await;
    let resources = progress_resources(Arc::new(|_, _| {
        Box::pin(async {
            Err(codex_protocol::error::CodexErr::InvalidRequest(
                "fixture failed to freeze progress".into(),
            ))
        })
    }));
    assert!(
        manager
            .get_thread(session.thread_id)
            .await
            .unwrap()
            .install_agent_host_resources(resources)
    );
    let handler = WaitAgentHandlerV2::default();
    tokio::time::pause();
    let error = tokio::time::timeout(
        Duration::from_secs(301),
        handler.handle(invocation(
            session,
            turn,
            "wait_agent",
            function_payload(json!({"wait_mode":"all","yield_time_ms":300000})),
        )),
    )
    .await
    .expect("progress failure should wake the wait")
    .err()
    .expect("expect a model-visible progress failure");
    tokio::time::resume();
    assert!(
        error
            .to_string()
            .contains("fixture failed to freeze progress")
    );
    assert_wait_item_settled(
        &events,
        codex_protocol::items::CollabAgentToolCallStatus::Failed,
    );
}

fn assert_wait_item_settled(
    events: &async_channel::Receiver<codex_protocol::protocol::Event>,
    expected: codex_protocol::items::CollabAgentToolCallStatus,
) {
    use codex_protocol::items::CollabAgentTool;
    use codex_protocol::items::CollabAgentToolCallStatus;
    use codex_protocol::items::TurnItem;

    let lifecycle = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| {
            let (starting, item) = match event.msg {
                EventMsg::ItemStarted(event) => (true, event.item),
                EventMsg::ItemCompleted(event) => (false, event.item),
                _ => return None,
            };
            match item {
                TurnItem::CollabAgentToolCall(item) if item.tool == CollabAgentTool::Wait => {
                    Some((starting, item.id, item.status))
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    let [
        (true, start, CollabAgentToolCallStatus::InProgress),
        (false, end, actual),
    ] = lifecycle.as_slice()
    else {
        panic!("expected one native wait start and one terminal item, got {lifecycle:?}");
    };
    assert_eq!(start, end);
    assert_eq!(actual, &expected);
}
