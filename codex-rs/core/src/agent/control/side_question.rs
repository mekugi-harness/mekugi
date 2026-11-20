//! Tool-free questions over a frozen recipient context. Ordinary BTW and progress
//! share this executor; callers supply already-acquired sources or range metadata.

use crate::codex_delegate::run_codex_thread_one_shot_with_tool_policy;
use crate::config::Config;
use crate::config::Constrained;
use crate::session::SessionIo;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_extension_api::ToolPolicy;
use codex_history::InitialHistory;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SubAgentSource;
use codex_user_input::ResolvedUserInput;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

impl super::LocalAgentRuntime {
    /// The caller authenticates local or foreign access before invoking this seam.
    /// This never loads a retained thread, sends primary input or acquires sources.
    pub(crate) async fn answer_recipient_side_question(
        &self,
        target: ThreadId,
        question: String,
        frozen_sources: Option<String>,
        cancellation: CancellationToken,
    ) -> CodexResult<String> {
        if question.trim().is_empty() {
            return Err(CodexErr::InvalidRequest(
                "side question must not be empty".into(),
            ));
        }
        self.ensure_agent_known(target)?;
        let thread = self.upgrade()?.get_thread(target).await?;
        let context = {
            let active = thread.session.active_turn.lock().await;
            active
                .as_ref()
                .and_then(|turn| turn.task.as_ref())
                .map(|task| Arc::clone(&task.turn_context))
        };
        let context = match context {
            Some(context) => context,
            None => thread.session.new_default_turn().await,
        };
        answer_side_question(
            Arc::clone(&thread.session),
            context,
            question,
            frozen_sources,
            None,
            cancellation,
        )
        .await
    }

    pub(crate) async fn answer_recipient_progress_question(
        &self,
        target: ThreadId,
        question: String,
        cancellation: CancellationToken,
    ) -> CodexResult<String> {
        self.ensure_agent_known(target)?;
        let thread = self.upgrade()?.get_thread(target).await?;
        let history = thread
            .session
            .progress_history_snapshot(&cancellation)
            .await?;
        let context = {
            let active = thread.session.active_turn.lock().await;
            active
                .as_ref()
                .and_then(|turn| turn.task.as_ref())
                .map(|task| Arc::clone(&task.turn_context))
        };
        let context = match context {
            Some(context) => context,
            None => thread.session.new_default_turn().await,
        };
        answer_side_question(
            Arc::clone(&thread.session),
            context,
            question,
            None,
            Some(history),
            cancellation,
        )
        .await
    }
}

async fn answer_side_question(
    target: Arc<Session>,
    context: Arc<TurnContext>,
    question: String,
    frozen_sources: Option<String>,
    history: Option<crate::context_manager::ContextManager>,
    cancellation: CancellationToken,
) -> CodexResult<String> {
    if cancellation.is_cancelled() {
        return Err(CodexErr::TurnAborted);
    }
    let runtime = &target.services.local_agent_runtime;
    let guard = runtime
        .admit_start()?
        .into_teardown_guard("side_question", Some(target.thread_id()));
    let shutdown = runtime.shutdown.clone();
    let cancellation = cancellation.child_token();
    // Abandoning the caller cancels only this request. The native tree membership
    // remains held by the cleanup task until private session/forwarders have joined.
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let (answer_tx, answer_rx) = oneshot::channel();
    tokio::spawn(async move {
        let run = async {
            let prepared =
                prepare_side_question(&target, &context, question, frozen_sources, history).await;
            let (_, io) = run_codex_thread_one_shot_with_tool_policy(
                prepared.config,
                Arc::clone(&target.services.auth_manager),
                Arc::clone(&target.services.models_manager),
                prepared.input,
                target,
                context,
                cancellation.clone(),
                SubAgentSource::Other("side_question".to_owned()),
                None,
                Some(prepared.history),
                Some(ToolPolicy {
                    allowed_tools: Some(Vec::new()),
                    ..Default::default()
                }),
            )
            .await?;
            collect_answer(io).await
        };
        tokio::pin!(run);
        let result = tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                cancellation.cancel();
                let _ = run.await;
                Err(CodexErr::TurnAborted)
            }
            result = &mut run => result,
        };
        let result = if cancellation.is_cancelled() {
            Err(CodexErr::TurnAborted)
        } else {
            result
        };
        let _ = answer_tx.send(result);
        guard.complete();
    });
    answer_rx.await.map_err(|_| CodexErr::InternalAgentDied)?
}

struct PreparedSideQuestion {
    config: Config,
    history: InitialHistory,
    input: Vec<ResolvedUserInput>,
}

async fn prepare_side_question(
    target: &Session,
    context: &TurnContext,
    question: String,
    frozen_sources: Option<String>,
    history: Option<crate::context_manager::ContextManager>,
) -> PreparedSideQuestion {
    let mut config = context.config.as_ref().clone();
    let (model, effort) = context.capture_current_model_settings();
    config.model = Some(model.slug.clone());
    config.model_reasoning_effort = effort;
    let instructions = target.get_base_instructions().await;
    config.base_instructions = Some(instructions.text);
    config.base_instructions_provenance = instructions.provenance;
    config.ephemeral = true;
    config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
    let history = match history {
        Some(history) => history,
        None => target.clone_history().await,
    };
    let mut history: Vec<_> = completed_tool_history(history.into_annotated_items())
        .into_iter()
        .map(RolloutItem::ResponseItem)
        .collect();
    history.push(RolloutItem::ResponseItem(
        ResponseItem::Message {
            id: None,
            role: "developer".to_owned(),
            content: vec![ContentItem::InputText {
                text: "Answer the question below from the supplied context.".to_owned(),
            }],
            status: None,
            encrypted_content: None,
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
        .into(),
    ));
    let mut input = vec![ResolvedUserInput::Text {
        text: question,
        text_elements: Vec::new(),
    }];
    if let Some(text) = frozen_sources {
        input.push(ResolvedUserInput::Text {
            text,
            text_elements: Vec::new(),
        });
    }
    PreparedSideQuestion {
        config,
        history: InitialHistory::Forked(history),
        input,
    }
}

/// Project a stable clone at completed-tool boundaries, not a synthetic aborted
/// output for an in-flight call. Keep every completed output verbatim, including
/// running-process and bounded-output continuation handles. No primary work waits.
fn completed_tool_history(history: Vec<ResponseItemEnvelope>) -> Vec<ResponseItemEnvelope> {
    crate::session::auxiliary_snapshot::completed_tool_items(&history)
        .cloned()
        .collect()
}

async fn collect_answer(io: SessionIo) -> CodexResult<String> {
    let result = loop {
        match io.next_event().await {
            Ok(event) => match event.msg {
                EventMsg::TurnComplete(complete) => {
                    break match complete.error {
                        Some(error) => Err(CodexErr::Fatal(error.message)),
                        None => complete
                            .last_agent_message
                            .filter(|answer| !answer.trim().is_empty())
                            .ok_or_else(|| {
                                CodexErr::InvalidRequest(
                                    "side question completed without an answer".to_owned(),
                                )
                            }),
                    };
                }
                EventMsg::TurnAborted(_) => break Err(CodexErr::TurnAborted),
                _ => {}
            },
            Err(error) => break Err(error),
        }
    };
    io.session_loop_termination.clone().await;
    result
}

#[cfg(test)]
#[path = "side_question_tests.rs"]
mod tests;
