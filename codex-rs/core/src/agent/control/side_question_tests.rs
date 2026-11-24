use super::*;
use crate::session::completed_session_loop_termination;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_protocol::protocol::TurnCompleteEvent;
use futures::FutureExt;
use pretty_assertions::assert_eq;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::watch;

fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_owned(),
        content: vec![ContentItem::InputText {
            text: text.to_owned(),
        }],
        status: None,
        encrypted_content: None,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn completed(answer: Option<&str>, error: Option<ErrorEvent>) -> EventMsg {
    EventMsg::TurnComplete(TurnCompleteEvent {
        root_turn_id: None,
        turn_id: "answer-turn".to_owned(),
        last_agent_message: answer.map(str::to_owned),
        error,
        started_at: None,
        completed_at: None,
        duration_ms: None,
        time_to_first_token_ms: None,
    })
}

fn answer_io() -> (async_channel::Sender<Event>, SessionIo) {
    let (events_tx, events_rx) = async_channel::bounded(4);
    let (tx_sub, rx_sub) = async_channel::bounded(1);
    drop(rx_sub);
    let (_status_tx, agent_status) = watch::channel(AgentStatus::PendingInit);
    (
        events_tx,
        SessionIo {
            tx_sub,
            rx_event: events_rx,
            agent_status,
            session_loop_termination: completed_session_loop_termination(),
        },
    )
}

#[tokio::test]
async fn preparation_freezes_native_history_and_preserves_original_input() {
    let (target, context, _events) =
        crate::session::tests::make_session_and_context_with_rx().await;
    target
        .record_conversation_items(
            &context,
            context.model_info(),
            &[user_message("existing target work")],
        )
        .await;
    let before = target.clone_history().await;
    let question = "question with @\"file name\" and literal $()";
    let frozen = "already acquired bytes, including a failed read";
    let prepared = prepare_side_question(
        &target,
        &context,
        question.to_owned(),
        Some(frozen.to_owned()),
        None,
    )
    .await;
    assert!(prepared.config.ephemeral);
    assert_eq!(
        prepared.config.permissions.approval_policy.value(),
        AskForApproval::Never,
    );
    assert_eq!(
        prepared.input,
        vec![
            ResolvedUserInput::Text {
                text: question.to_owned(),
                text_elements: Vec::new()
            },
            ResolvedUserInput::Text {
                text: frozen.to_owned(),
                text_elements: Vec::new()
            },
        ],
    );
    assert_eq!(
        target.clone_history().await.history_version(),
        before.history_version(),
    );
    assert_eq!(
        target.clone_history().await.annotated_items(),
        before.annotated_items(),
    );
    target
        .record_conversation_items(
            &context,
            context.model_info(),
            &[user_message("later target input")],
        )
        .await;
    let InitialHistory::Forked(items) = prepared.history else {
        panic!("side question must use native fork initialization");
    };
    let history: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => Some(envelope),
            _ => None,
        })
        .collect();
    assert_eq!(history.len(), before.annotated_items().len() + 1);
    for (actual, expected) in history.iter().zip(before.annotated_items()) {
        assert_eq!(*actual, expected);
    }
    assert!(matches!(
        &history.last().unwrap().item,
        ResponseItem::Message { role, .. } if role == "developer"
    ));
    let second = prepare_side_question(
        &target,
        &context,
        "second independent question".to_owned(),
        None,
        None,
    )
    .await;
    assert_eq!(second.input.len(), 1);
    assert_eq!(
        second.history.get_rollout_items().len(),
        target.clone_history().await.annotated_items().len() + 1,
    );
}

#[tokio::test]
async fn terminal_answer_waits_for_private_execution_to_join() {
    let (events, mut io) = answer_io();
    let (joined_tx, joined_rx) = oneshot::channel();
    io.session_loop_termination = async move { joined_rx.await.unwrap() }.boxed().shared();
    events
        .send(Event {
            id: "terminal".to_owned(),
            msg: completed(Some("answer"), None),
        })
        .await
        .unwrap();
    let mut answer = Box::pin(collect_answer(io));
    assert!(matches!(futures::poll!(answer.as_mut()), Poll::Pending));
    joined_tx.send(()).unwrap();
    assert_eq!(answer.await.unwrap(), "answer");
}

#[tokio::test]
async fn failed_partial_and_missing_answers_are_not_successful_results() {
    for event in [
        completed(
            Some("partial"),
            Some(ErrorEvent {
                message: "provider failed".to_owned(),
                codex_error_info: None,
                rejected_audio_file_ids: Vec::new(),
                misalignment: None,
            }),
        ),
        completed(None, None),
        completed(Some("  \n"), None),
        EventMsg::TurnAborted(TurnAbortedEvent {
            root_turn_id: None,
            turn_id: Some("answer-turn".to_owned()),
            started_at: None,
            reason: TurnAbortReason::Interrupted,
            error: None,
            completed_at: None,
            duration_ms: None,
        }),
    ] {
        let (events, io) = answer_io();
        events
            .send(Event {
                id: "terminal".to_owned(),
                msg: event,
            })
            .await
            .unwrap();
        assert!(collect_answer(io).await.is_err());
    }
    let (events, io) = answer_io();
    drop(events);
    assert!(matches!(
        collect_answer(io).await.unwrap_err().details(),
        CodexErrorDetails::InternalAgentDied,
    ));
}

#[test]
fn completed_tools_keep_outputs_and_continuations_without_pending_calls() {
    let inputs = [
        serde_json::json!({"type":"function_call","call_id":"done","name":"exec","arguments":"{}"}),
        serde_json::json!({"type":"custom_tool_call","call_id":"pending","name":"exec","input":"{}"}),
        serde_json::json!({"type":"function_call_output","call_id":"done","output":"session_id=42; next_call: mread retained"}),
        serde_json::json!({"type":"tool_search_call","call_id":"search","execution":"client","arguments":{}}),
        serde_json::json!({"type":"custom_tool_call","call_id":"custom","name":"exec","input":"{}"}),
        serde_json::json!({"type":"custom_tool_call_output","call_id":"custom","output":"complete"}),
    ];
    let items: Vec<ResponseItemEnvelope> = inputs
        .into_iter()
        .map(|value| {
            serde_json::from_value::<ResponseItem>(value)
                .unwrap()
                .into()
        })
        .collect();
    let expected = vec![
        items[0].clone(),
        items[2].clone(),
        items[4].clone(),
        items[5].clone(),
    ];
    assert_eq!(completed_tool_history(items), expected);
}

#[tokio::test]
async fn cancelled_question_does_not_start_or_modify_primary_work() {
    let (target, context, _events) =
        crate::session::tests::make_session_and_context_with_rx().await;
    let before = target.clone_history().await.history_version();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = answer_side_question(
        Arc::clone(&target),
        context,
        "question".into(),
        None,
        None,
        cancellation,
    )
    .await
    .unwrap_err();
    assert!(matches!(error.details(), CodexErrorDetails::TurnAborted));
    assert_eq!(target.clone_history().await.history_version(), before);
}

#[tokio::test]
async fn cancelled_auxiliary_joins_without_cancelling_primary_or_sibling() {
    let (target, context, _events) =
        crate::session::tests::make_session_and_context_with_rx().await;
    let before = target.clone_history().await.history_version();
    let cancellation = CancellationToken::new();
    let sibling = CancellationToken::new();
    let mut answer = Box::pin(answer_side_question(
        Arc::clone(&target),
        context,
        "question".into(),
        None,
        None,
        cancellation.clone(),
    ));
    assert!(matches!(futures::poll!(answer.as_mut()), Poll::Pending));
    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(10), answer)
        .await
        .expect("auxiliary cleanup must settle");
    assert!(matches!(
        result.unwrap_err().details(),
        CodexErrorDetails::TurnAborted
    ));
    assert!(!sibling.is_cancelled());
    assert!(!target.services.local_agent_runtime.shutdown.is_cancelled());
    assert_eq!(target.clone_history().await.history_version(), before);
    target
        .services
        .local_agent_runtime
        .request_shutdown()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn preparation_downgrades_ultra_reasoning_effort_to_max() {
    let (target, context, _events) =
        crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
            codex_login::CodexAuth::from_api_key("Test API Key"),
            Vec::new(),
            |config| {
                config.model_reasoning_effort =
                    Some(codex_protocol::openai_models::ReasoningEffort::Ultra);
            },
        )
        .await;
    let prepared = prepare_side_question(&target, &context, "status?".to_owned(), None, None).await;
    assert_eq!(
        prepared.config.model_reasoning_effort,
        Some(codex_protocol::openai_models::ReasoningEffort::Max),
    );
}
