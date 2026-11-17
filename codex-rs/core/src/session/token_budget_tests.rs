use super::*;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;

fn user() -> ResponseItem {
    serde_json::from_value(serde_json::json!({"type":"message", "role":"user", "content":[{"type":"input_text","text":"continue"}]})).unwrap()
}

fn tool() -> ResponseItem {
    ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: "call".into(),
        name: Some("exec".into()),
        output: FunctionCallOutputPayload::from_text("done".into()),
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn milestones_follow_new_boundaries_and_survive_history_reconstruction() {
    let mut history = vec![user()];
    assert!(notice_due(history.iter(), 499, 1000).is_none());
    assert!(notice_due(history.iter(), 999, 0).is_none());
    for tokens in [500, 600, 700, 800, 950] {
        let (level, reminder) = notice_due(history.iter(), tokens, 1000).unwrap();
        assert_eq!(level, (tokens / 100 * 10).min(90));
        history.push(ContextualUserFragment::into(reminder));
        assert!(notice_due(history.iter(), tokens, 1000).is_none());
        // Serialize/reload the real native items, as retained history does.
        history = serde_json::from_slice(&serde_json::to_vec(&history).unwrap()).unwrap();
        assert!(notice_due(history.iter(), tokens + 10, 1000).is_none());
        history.push(tool());
    }
    assert!(notice_due(history.iter(), 1000, 1000).is_none());
    // A new context or independent child has no inherited notice ledger.
    assert!(notice_due([user()].iter(), 900, 1000).is_some());
    assert!(notice_due([].iter(), 900, 1000).is_none());
}

#[tokio::test]
async fn native_history_records_typed_notice_and_emits_only_fresh_raw_item() {
    use crate::session::tests::make_session_and_context_with_rx;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::TokenUsage;
    use codex_protocol::protocol::TokenUsageInfo;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let (session, turn, rx) = make_session_and_context_with_rx().await;
    let step = session
        .capture_step_context(Arc::clone(&turn), &CancellationToken::new())
        .await
        .unwrap();
    session
        .record_conversation_items(&turn, &step.settings.model_info, &[user()])
        .await;
    // Unknown usage never generates a notice.
    maybe_record_context_usage(&session, &step).await;
    let window = step.settings.model_info.usable_context_window().unwrap();
    {
        let mut state = session.state.lock().await;
        state.set_token_info(Some(TokenUsageInfo {
            total_token_usage: TokenUsage::default(),
            last_token_usage: TokenUsage {
                total_tokens: window * 95 / 100,
                ..Default::default()
            },
            last_usage_includes_reasoning: Some(true),
            model_context_window: Some(window),
        }));
    }
    while rx.try_recv().is_ok() {}
    maybe_record_context_usage(&session, &step).await;
    maybe_record_context_usage(&session, &step).await;
    let history = session.clone_history().await;
    let notices: Vec<_> = history.raw_items().filter(|item| matches!(item,
        ResponseItem::Message { role, internal_chat_message_metadata_passthrough: Some(metadata), .. }
        if role == "developer" && metadata.content_item_kinds.as_ref().is_some_and(|kinds| kinds.iter().any(|k| k.0.starts_with(KIND_PREFIX)))
    )).collect();
    assert_eq!(notices.len(), 1);
    assert!(
        serde_json::to_string(notices[0])
            .unwrap()
            .contains("You must compact now.")
    );
    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event.msg {
            EventMsg::RawResponseItem(raw) => Some(raw.item),
            _ => None,
        })
        .collect();
    assert_eq!(events, vec![notices[0].clone()]);
    assert!(session.state.lock().await.completed_compactions == Some(0));

    // A full-history child retains parent notices but owns its own cadence.
    let (child, child_turn, _) = make_session_and_context_with_rx().await;
    let child_step = child
        .capture_step_context(Arc::clone(&child_turn), &CancellationToken::new())
        .await
        .unwrap();
    child
        .replace_history(history.raw_items().cloned().collect(), None)
        .await;
    child
        .record_conversation_items(&child_turn, &child_step.settings.model_info, &[user()])
        .await;
    child
        .state
        .lock()
        .await
        .set_token_info(session.state.lock().await.token_info());
    maybe_record_context_usage(&child, &child_step).await;
    let child_history = child.clone_history().await;
    assert_eq!(child_history.raw_items().filter(|item| matches!(item, ResponseItem::Message { id: Some(id), .. } if id.starts_with("context-usage_"))).count(), 2);

    // Guardian approval sessions use ordinary turns but must receive no notice.
    let (approval, mut approval_turn, _) = make_session_and_context_with_rx().await;
    Arc::get_mut(&mut approval_turn).unwrap().session_source =
        codex_protocol::protocol::SessionSource::SubAgent(
            codex_protocol::protocol::SubAgentSource::Other("guardian".into()),
        );
    let approval_step = approval
        .capture_step_context(Arc::clone(&approval_turn), &CancellationToken::new())
        .await
        .unwrap();
    approval
        .record_conversation_items(
            &approval_turn,
            &approval_step.settings.model_info,
            &[user()],
        )
        .await;
    approval
        .state
        .lock()
        .await
        .set_token_info(session.state.lock().await.token_info());
    maybe_record_context_usage(&approval, &approval_step).await;
    assert_eq!(approval.clone_history().await.raw_items().count(), 1);
}

#[test]
fn milestone_scan_skips_below_threshold_and_stops_at_retained_notice() {
    let notice = ContextualUserFragment::into(TokenBudgetReminder::context_usage(60, 600, 1000));
    let history = [user(), notice];
    let visits = std::cell::Cell::new(0);
    let items = || history.iter().inspect(|_| visits.set(visits.get() + 1));
    assert!(notice_due(items(), 499, 1000).is_none());
    assert_eq!(visits.get(), 0);
    assert!(notice_due(items(), 600, 1000).is_none());
    assert_eq!(visits.get(), 1);
}

#[test]
fn reminder_renderer_preserves_remaining_token_template_and_usage_notice() {
    let reminder = TokenBudgetReminder::new("Remaining: {n_remaining}", 42);
    assert_eq!(
        reminder.render(),
        codex_protocol::models::input_text("Remaining: 42")
    );
    assert_eq!(reminder.content_kind().0, "token_budget.reminder");
    let notice = TokenBudgetReminder::context_usage(95, 950, 1000);
    assert_eq!(
        notice.render(),
        codex_protocol::models::input_text(
            "Mekugi context usage: 95% (950/1000 tokens). Your context window is nearly exhausted (only 50 tokens remaining). You must compact now."
        )
    );
    assert_eq!(notice.content_kind().0, "context_usage.notice.90");
    assert_eq!(
        TokenBudgetReminder::context_usage(60, 600, 1000).render(),
        codex_protocol::models::input_text("Mekugi context usage: 60% (600/1000 tokens).")
    );
    assert!(
        !serde_json::to_string(&notice.render())
            .unwrap()
            .contains("system-reminder")
    );
}
