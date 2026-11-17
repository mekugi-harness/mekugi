use super::session::Session;
use super::step_context::StepContext;
use super::turn_context::TurnContext;
use crate::context::ContextualUserFragment;
use crate::context::TokenBudgetReminder;
use crate::context_manager::is_user_turn_boundary;
use codex_features::Feature;
use codex_protocol::models::ResponseItem;

pub(super) async fn maybe_record(
    sess: &Session,
    turn_context: &TurnContext,
    base_window_tokens_remaining: Option<i64>,
    allow_auto_compact_fallback: bool,
) {
    if !turn_context.config.features.enabled(Feature::TokenBudget) {
        return;
    }
    let Some(base_window_tokens_remaining) = base_window_tokens_remaining else {
        return;
    };

    let Some(config) = turn_context.config.token_budget.as_ref() else {
        return;
    };

    if config
        .reminder_threshold_tokens
        .is_some_and(|threshold| base_window_tokens_remaining <= threshold)
    {
        let reminder_due = {
            let mut state = sess.state.lock().await;
            state.claim_token_budget_reminder()
        };
        if reminder_due {
            let response_item =
                ContextualUserFragment::into(crate::context::TokenBudgetReminder::new(
                    &config.reminder_message_template,
                    base_window_tokens_remaining,
                ));
            sess.record_conversation_items(
                turn_context,
                turn_context.model_info(),
                std::slice::from_ref(&response_item),
            )
            .await;
        }
    }

    if !allow_auto_compact_fallback || base_window_tokens_remaining != 0 {
        return;
    }
    let Some(prompt) = config.auto_compact_fallback_prompt.as_deref() else {
        return;
    };

    let fallback_due = {
        let mut state = sess.state.lock().await;
        state.claim_auto_compact_fallback()
    };
    if !fallback_due {
        return;
    }

    let response_item =
        ContextualUserFragment::into(crate::context::AutoCompactFallbackPrompt::new(prompt));
    sess.record_conversation_items(
        turn_context,
        turn_context.model_info(),
        std::slice::from_ref(&response_item),
    )
    .await;
}

// Keep the persisted usage-notice classification stable across resume.
const KIND_PREFIX: &str = TokenBudgetReminder::USAGE_KIND_PREFIX;

fn notice_due<'a>(
    history: impl DoubleEndedIterator<Item = &'a ResponseItem>,
    tokens: i64,
    window: i64,
) -> Option<(i64, TokenBudgetReminder)> {
    if window <= 0 || tokens < 0 {
        return None;
    }
    let percent = ((i128::from(tokens.min(window)) * 100) / i128::from(window)) as i64;
    let level = (percent / 10 * 10).min(90);
    if level < 50 {
        return None;
    }
    let mut boundary = false;
    // Borrow only, newest-first; stop at the previous notice. No separate per-thread ledger,
    // payload clones, or provider-prefix reconstruction. Reset/fork/resume use native history.
    for item in history.rev() {
        if let ResponseItem::Message {
            role,
            internal_chat_message_metadata_passthrough: Some(metadata),
            ..
        } = item
            && role == "developer"
            && let Some(previous_level) = metadata.content_item_kinds.as_ref().and_then(|kinds| {
                kinds
                    .iter()
                    .find_map(|kind| kind.0.strip_prefix(KIND_PREFIX)?.parse::<i64>().ok())
            })
        {
            if previous_level >= level {
                return None;
            }
            break;
        }
        boundary |= is_user_turn_boundary(item)
            || matches!(
                item,
                ResponseItem::FunctionCallOutput { .. }
                    | ResponseItem::CustomToolCallOutput { .. }
                    | ResponseItem::ToolSearchOutput { .. }
            );
    }
    boundary.then(|| {
        (
            level,
            TokenBudgetReminder::context_usage(percent, tokens, window),
        )
    })
}

#[tracing::instrument(skip_all)]
pub(super) async fn maybe_record_context_usage(sess: &Session, step: &StepContext) {
    if crate::guardian::is_basic_session_source(&step.turn.session_source) {
        return;
    }
    let origin_prefix = format!("context-usage_{}-", sess.thread_id());
    let notice = {
        let state = sess.state.lock().await;
        let Some(info) = state.token_info() else {
            return;
        };
        let Some(window) = info
            .model_context_window
            .or_else(|| step.settings.model_info.usable_context_window())
        else {
            return;
        };
        notice_due(
            state.history.raw_items().filter(|item| {
                // Full-history children retain the parent's content, not its milestone ledger.
                !matches!(item, ResponseItem::Message { id: Some(id), .. }
                    if id.starts_with("context-usage_") && !id.starts_with(&origin_prefix))
            }),
            info.last_token_usage.total_tokens,
            window,
        )
    };
    if let Some((level, reminder)) = notice {
        let window_id = sess.current_window_id().await;
        let mut item = ContextualUserFragment::into(reminder);
        if let ResponseItem::Message { id, .. } = &mut item {
            *id = Some(codex_protocol::ResponseItemId::with_suffix(
                "context-usage",
                format!("{}-{window_id}-{level}", sess.thread_id()),
            ));
        }
        sess.record_conversation_items(
            step.turn.as_ref(),
            step.settings.model_info.as_ref(),
            &[item],
        )
        .await;
    }
}

#[cfg(test)]
#[path = "token_budget_tests.rs"]
mod tests;
