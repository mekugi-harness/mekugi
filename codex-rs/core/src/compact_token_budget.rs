use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::context::world_state::WorldState;
use crate::hook_runtime::PostCompactHookOutcome;
use crate::hook_runtime::PreCompactHookOutcome;
use crate::hook_runtime::run_post_compact_hooks;
use crate::hook_runtime::run_pre_compact_hooks;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use codex_analytics::CompactionTrigger;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;

/// Runs token-budget manual compaction as a normal compaction lifecycle.
///
/// Token-budget compaction skips model/server summarization and installs a fresh context window
/// instead. It is still modeled as compaction so compact hooks and `ContextCompaction` turn items
/// observe the same lifecycle as local or remote compaction.
pub(crate) async fn run_manual_compact_task(
    sess: Arc<Session>,
    step_context: Arc<StepContext>,
    world_state: Arc<WorldState>,
    cancellation_token: &CancellationToken,
) -> CodexResult<()> {
    run_compact_task_inner(
        &sess,
        &step_context,
        world_state,
        CompactionTrigger::Manual,
        cancellation_token,
    )
    .await
}

/// Runs token-budget inline auto-compaction as a normal compaction lifecycle.
///
/// Token-budget compaction skips model/server summarization and installs a fresh context window
/// instead. It is still modeled as compaction so compact hooks and `ContextCompaction` turn items
/// observe the same lifecycle as local or remote compaction.
pub(crate) async fn run_inline_auto_compact_task(
    sess: Arc<Session>,
    step_context: Arc<StepContext>,
    world_state: Arc<WorldState>,
    cancellation_token: &CancellationToken,
) -> CodexResult<()> {
    run_compact_task_inner(
        &sess,
        &step_context,
        world_state,
        CompactionTrigger::Auto,
        cancellation_token,
    )
    .await
}

async fn run_compact_task_inner(
    sess: &Arc<Session>,
    step_context: &Arc<StepContext>,
    world_state: Arc<WorldState>,
    trigger: CompactionTrigger,
    cancellation_token: &CancellationToken,
) -> CodexResult<()> {
    let turn_context = &step_context.turn;
    let pre_compact_outcome = run_pre_compact_hooks(sess, turn_context, trigger).await;
    match pre_compact_outcome {
        PreCompactHookOutcome::Continue => {}
        PreCompactHookOutcome::Stopped => return Err(CodexErr::TurnAborted),
    }

    let mut compaction = ContextCompactionItem::new();
    compaction.summary_free = true;
    sess.emit_turn_item_started(
        turn_context,
        &TurnItem::ContextCompaction(compaction.clone()),
    )
    .await;
    let reload = match sess.reload_selection().await {
        Some(selection) => {
            let mut sources = Vec::new();
            crate::context_sources::acquire_selection(
                sess,
                step_context,
                &selection,
                cancellation_token,
                Some(&mut sources),
            )
            .await?
            .map(|text| {
                (
                    text,
                    codex_protocol::protocol::SourceAcquisition {
                        id: compaction.id.clone(),
                        purpose: "reload".to_string(),
                        sources,
                    },
                )
            })
        }
        None => None,
    };
    if cancellation_token.is_cancelled() {
        return Err(CodexErr::TurnAborted);
    }
    compaction.source_acquisition = reload.as_ref().map(|(_, receipt)| receipt.clone());
    // This reset installs history without making a remote compaction request.
    let trace = sess.services.rollout_thread_trace.compaction_trace_context(
        &turn_context.sub_id,
        &compaction.id,
        &turn_context.model_info().slug,
        &turn_context.provider.info().name,
    );
    sess.start_new_context_window(step_context, world_state, reload, &trace)
        .await;
    sess.emit_turn_item_completed(turn_context, TurnItem::ContextCompaction(compaction))
        .await;

    let post_compact_outcome = run_post_compact_hooks(sess, turn_context, trigger).await;
    if let PostCompactHookOutcome::Stopped = post_compact_outcome {
        return Err(CodexErr::TurnAborted);
    }

    Ok(())
}
