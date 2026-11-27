use codex_protocol::AgentPath;
use codex_protocol::ResponseItemId;
use codex_protocol::ThreadId;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::AgentMessageItem;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::RawResponseItemEvent;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::security_risk::SecurityRiskScore;
use codex_protocol::user_input::UserInput;
use codex_rollout::CompactedItem;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutLine;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

use super::*;
use crate::protocol::thread_history::ThreadHistoryBuilder;
use crate::protocol::v2::AgentMailKind;
use crate::protocol::v2::ThreadItemPayload;
use crate::protocol::v2::TurnError;

#[test]
fn projects_one_authored_reply_from_live_and_persisted_mail() {
    let body = "## Summary\nFound the cause.\n\n~~~markdown\n## Not a boundary\n~~~\n\n\
                ## Details\nThe worker verified the release gate.";
    let mut reply = InterAgentCommunication::new(
        AgentPath::root().join("worker").unwrap(),
        AgentPath::root(),
        Vec::new(),
        format!(
            "Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/worker\nPayload:\n{body}"
        ),
        /*trigger_turn*/ false,
    );
    reply.id = Some(ResponseItemId::with_suffix("amsg", "reply"));
    let mut item = reply.to_model_input_item();
    item.set_turn_id_if_missing("receiving-turn");
    item.set_create_time_if_missing(serde_json::Number::from_f64(1.25).unwrap());
    let ResponseItem::AgentMessage { content, .. } = &mut item else {
        panic!("expected a retained mail item");
    };
    content.push(AgentMessageInputContent::InputText {
        text: "Attached model context, not the authored reply.".to_owned(),
    });

    let expected = ThreadItem {
        started_at_ms: Some(1250),
        completed_at_ms: Some(1250),
        payload: ThreadItemPayload::AgentMail {
            id: "amsg_reply".to_owned(),
            kind: AgentMailKind::Reply,
            sender: "/root/worker".to_owned(),
            recipient: "/root".to_owned(),
            text: body.to_owned(),
            summary: Some("Found the cause.\n\n~~~markdown\n## Not a boundary\n~~~".to_owned()),
            details: Some("The worker verified the release gate.".to_owned()),
        },
    };
    let record = RolloutItem::ResponseItem(item.clone().into());
    let notification = EventMsg::RawResponseItem(RawResponseItemEvent { item });
    assert_eq!(
        project(record.clone()).changed_items,
        vec![ThreadHistoryItemChange {
            turn_id: "receiving-turn".to_owned(),
            item: expected.clone(),
        }]
    );
    let mut history = ThreadHistoryBuilder::new();
    history.handle_event(&EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: "receiving-turn".to_owned(),
        turn_attribution: None,
        root_turn_id: None,
        trace_id: None,
        started_at: Some(1),
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    }));
    assert_eq!(
        history
            .handle_event_with_changes(&notification)
            .changed_items[0]
            .item,
        expected
    );
    history.handle_rollout_item(&record);
    assert_eq!(history.finish()[0].items, vec![expected]);
}

#[test]
fn projects_consumed_message_without_treating_accepted_or_encrypted_mail_as_delivery() {
    let mut message = InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::root().join("worker").unwrap(),
        Vec::new(),
        "Message Type: MESSAGE\nTask name: /root/worker\nSender: /root\nPayload:\nFirst line.\nSecond line."
            .to_owned(),
        /*trigger_turn*/ false,
    );
    message.id = Some(ResponseItemId::with_suffix("amsg", "message"));
    let mut item = message.to_model_input_item();
    assert!(ThreadItem::from_delivered_agent_mail(&item).is_none());

    item.set_turn_id_if_missing("receiving-turn");
    assert_eq!(
        ThreadItem::from_delivered_agent_mail(&item),
        Some((
            "receiving-turn",
            ThreadItem::untimed(ThreadItemPayload::AgentMail {
                id: "amsg_message".to_owned(),
                kind: AgentMailKind::Message,
                sender: "/root".to_owned(),
                recipient: "/root/worker".to_owned(),
                text: "First line.\nSecond line.".to_owned(),
                summary: None,
                details: None,
            }),
        ))
    );
    let ResponseItem::AgentMessage { content, .. } = &mut item else {
        panic!("expected a retained mail item");
    };
    content.push(AgentMessageInputContent::EncryptedContent {
        encrypted_content: "private".to_owned(),
    });
    assert!(ThreadItem::from_delivered_agent_mail(&item).is_none());
}

#[test]
fn projects_turn_lifecycle_without_prior_builder_state() {
    let started = project(RolloutItem::EventMsg(EventMsg::TurnStarted(
        TurnStartedEvent {
            turn_attribution: None,
            turn_id: "turn-1".to_string(),
            root_turn_id: Some("root-turn".into()),
            trace_id: None,
            started_at: Some(10),
            model_context_window: None,
            collaboration_mode_kind: Default::default(),
        },
    )));
    let completed = project(RolloutItem::EventMsg(EventMsg::TurnComplete(
        TurnCompleteEvent {
            root_turn_id: Some("root-turn".into()),
            turn_id: "turn-1".to_string(),
            last_agent_message: None,
            error: None,
            started_at: Some(10),
            completed_at: Some(20),
            duration_ms: Some(10_000),
            time_to_first_token_ms: None,
        },
    )));

    assert_eq!(started.changed_turns.len(), 1);
    assert_eq!(started.changed_turns[0].turn_id, "turn-1");
    assert_eq!(started.changed_turns[0].status, TurnStatus::InProgress);
    assert_eq!(started.changed_turns[0].started_at, Some(10));
    assert_eq!(
        started.changed_turns[0].root_turn_id.as_deref(),
        Some("root-turn")
    );
    assert_eq!(
        completed,
        ThreadHistoryChangeSet {
            changed_turns: vec![ThreadHistoryTurnMetadata {
                turn_id: "turn-1".to_string(),
                root_turn_id: Some("root-turn".into()),
                status: TurnStatus::Completed,
                error: None,
                started_at: Some(10),
                completed_at: Some(20),
                duration_ms: Some(10_000),
            }],
            ..Default::default()
        }
    );
}

#[test]
fn projects_failed_turn_completion_as_snapshot() {
    let error = ErrorEvent {
        misalignment: None,
        message: "request failed".to_string(),
        codex_error_info: None,
        rejected_audio_file_ids: Vec::new(),
    };

    let changes = project(RolloutItem::EventMsg(EventMsg::TurnComplete(
        TurnCompleteEvent {
            root_turn_id: None,
            turn_id: "turn-1".to_string(),
            last_agent_message: None,
            error: Some(error),
            started_at: Some(10),
            completed_at: Some(20),
            duration_ms: Some(10_000),
            time_to_first_token_ms: None,
        },
    )));

    assert_eq!(
        changes,
        ThreadHistoryChangeSet {
            changed_turns: vec![ThreadHistoryTurnMetadata {
                turn_id: "turn-1".to_string(),
                root_turn_id: None,
                status: TurnStatus::Failed,
                error: Some(TurnError {
                    misalignment: None,
                    message: "request failed".to_string(),
                    codex_error_info: None,
                    additional_details: None,
                }),
                started_at: Some(10),
                completed_at: Some(20),
                duration_ms: Some(10_000),
            }],
            ..Default::default()
        }
    );
}

#[test]
fn projects_completed_canonical_turn_items() {
    let thread_id = ThreadId::default();
    let user_item = TurnItem::UserMessage(UserMessageItem {
        id: "user-1".to_string(),
        client_id: None,
        content: vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }],
    });
    let agent_item = TurnItem::AgentMessage(AgentMessageItem {
        moderation: None,
        id: "agent-1".to_string(),
        content: vec![AgentMessageContent::Text {
            text: "done".to_string(),
        }],
        phase: None,
        memory_citation: None,
        delivery: None,
        questions: None,
    });

    let user_changes = project(item_completed(thread_id, "turn-1", user_item.clone()));
    let agent_changes = project(item_completed(thread_id, "turn-1", agent_item.clone()));

    assert_eq!(
        user_changes.changed_items,
        vec![ThreadHistoryItemChange {
            turn_id: "turn-1".to_string(),
            item: ThreadItem {
                payload: ThreadItemPayload::from(user_item),
                started_at_ms: Some(100),
                completed_at_ms: Some(123)
            }
        }]
    );
    assert_eq!(
        agent_changes.changed_items,
        vec![ThreadHistoryItemChange {
            turn_id: "turn-1".to_string(),
            item: ThreadItem {
                payload: ThreadItemPayload::from(agent_item),
                started_at_ms: Some(100),
                completed_at_ms: Some(123)
            }
        }]
    );
}

#[test]
fn projects_optional_completed_item_lifecycle_timestamps() {
    let item = TurnItem::UserMessage(UserMessageItem {
        id: "user-1".to_string(),
        client_id: None,
        content: Vec::new(),
    });

    for (started_at_ms, completed_at_ms, expected_completed_at_ms) in
        [(None, 123, Some(123)), (Some(100), 0, None)]
    {
        let changes = project(RolloutItem::EventMsg(EventMsg::ItemCompleted(
            ItemCompletedEvent {
                content_token_count: None,
                thread_id: ThreadId::default(),
                turn_id: "turn-1".to_string(),
                item: item.clone(),
                started_at_ms,
                completed_at_ms,
            },
        )));

        assert_eq!(
            changes,
            ThreadHistoryChangeSet {
                changed_items: vec![ThreadHistoryItemChange {
                    turn_id: "turn-1".to_string(),
                    item: ThreadItem {
                        payload: ThreadItemPayload::from(item.clone()),
                        started_at_ms,
                        completed_at_ms: expected_completed_at_ms
                    }
                }],
                ..Default::default()
            }
        );
    }
}

#[test]
fn ignores_legacy_abort_without_turn_id_and_context_only_records() {
    let aborted = project(RolloutItem::EventMsg(EventMsg::TurnAborted(
        TurnAbortedEvent {
            root_turn_id: None,
            turn_id: None,
            reason: TurnAbortReason::Interrupted,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        },
    )));
    let compacted = project(RolloutItem::Compacted(Box::new(CompactedItem {
        message: String::new(),
        replacement_history: None,
        retained_context: None,
        guardian_history: None,
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
        compaction_response_id: None,
        latest_token_usage_record: None,
        resume_metadata: None,
    })));
    let security_risk = project(RolloutItem::SecurityRiskScore(SecurityRiskScore {
        scores: BTreeMap::from([("action_risk".to_string(), 0.92)]),
        call_id: None,
        action: None,
        sampled_at: None,
    }));

    assert!(aborted.is_empty());
    assert!(compacted.is_empty());
    assert!(security_risk.is_empty());
}

#[test]
fn projects_identified_turn_aborts() {
    let changes = project(RolloutItem::EventMsg(EventMsg::TurnAborted(
        TurnAbortedEvent {
            root_turn_id: Some("root-turn".into()),
            turn_id: Some("turn-1".to_string()),
            reason: TurnAbortReason::Interrupted,
            error: None,
            started_at: Some(10),
            completed_at: Some(20),
            duration_ms: Some(10_000),
        },
    )));

    assert_eq!(
        changes,
        ThreadHistoryChangeSet {
            changed_turns: vec![ThreadHistoryTurnMetadata {
                turn_id: "turn-1".to_string(),
                root_turn_id: Some("root-turn".into()),
                status: TurnStatus::Interrupted,
                error: None,
                started_at: Some(10),
                completed_at: Some(20),
                duration_ms: Some(10_000),
            }],
            ..Default::default()
        }
    );
}

fn project(item: RolloutItem) -> ThreadHistoryChangeSet {
    project_rollout_line(&RolloutLine {
        timestamp: "2026-07-09T00:00:00.000Z".to_string(),
        ordinal: Some(7),
        item,
    })
}

fn item_completed(thread_id: ThreadId, turn_id: &str, item: TurnItem) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        content_token_count: None,
        thread_id,
        turn_id: turn_id.to_string(),
        item,
        started_at_ms: Some(100),
        completed_at_ms: 123,
    }))
}
