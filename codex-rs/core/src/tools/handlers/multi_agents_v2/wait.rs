use super::*;
use crate::session::InputQueueActivity;
use crate::session::session::Session;
use crate::thread_input::pending_wait_activity;
use crate::thread_input::subscribe_input_activity;
use crate::tools::handlers::multi_agents_spec::WaitAgentTimeoutOptions;
use crate::tools::handlers::multi_agents_spec::create_wait_agent_tool_v2;
use codex_protocol::ThreadId;
use codex_protocol::agent_capture::PreparedProgressAgent;
use codex_protocol::agent_capture::ProgressActor;
use codex_protocol::agent_capture::ProgressOutputResponse;
use codex_protocol::agent_capture::SummaryResult;
use codex_tools::ToolSpec;
use futures::StreamExt;
use futures::stream::SelectAll;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct Handler {
    options: WaitAgentTimeoutOptions,
}

impl Handler {
    pub(crate) fn new(options: WaitAgentTimeoutOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("wait_agent")
    }

    fn spec(&self) -> ToolSpec {
        create_wait_agent_tool_v2(self.options)
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl Handler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            cancellation_token,
            source,
            ..
        } = invocation;
        let arguments = function_arguments(payload)?;
        let args: WaitArgs = parse_arguments(&arguments)?;
        if args.yield_time_ms.is_some_and(|ms| ms < 300_000) {
            return Err(FunctionCallError::RespondToModel(
                "yield_time_ms must be at least 300000".to_string(),
            ));
        }

        // Subscribe before observing pending mail or child status so selection cannot
        // lose a completion arriving while its initial snapshot is being acquired.
        let (mut activity_rx, _) = subscribe_input_activity(&session, &turn.sub_id).await;
        let pending = pending_wait_activity(&session, &turn.sub_id).await;
        let mut selected = pending.results.clone();
        let mut completed = pending.results;
        let control = session
            .services
            .local_agent_runtime
            .control(session.session_id());
        let current_path = turn
            .session_source
            .get_agent_path()
            .unwrap_or_else(codex_protocol::AgentPath::root);
        let mut statuses = SelectAll::new();
        let mut progress_actors = Vec::new();
        for agent in control
            .list_agents(&turn.session_source, Some(current_path.as_str()))
            .await
            .map_err(collab_spawn_error)?
        {
            if agent.thread_id == session.thread_id
                || !matches!(
                    agent.status,
                    AgentStatus::PendingInit | AgentStatus::Running
                )
            {
                continue;
            }
            let id = agent.thread_id;
            selected.insert(id);
            progress_actors.push(ProgressActor {
                thread_id: id,
                task_name: agent
                    .metadata
                    .agent_path
                    .map(|path| path.to_string())
                    .unwrap_or_else(|| id.to_string()),
            });
            match control.subscribe_status(id).await {
                Ok(updates) => statuses.push(
                    updates
                        .map(move |update| {
                            let status = update
                                .ok()
                                .and_then(|info| info.status().cloned())
                                .unwrap_or(AgentStatus::NotFound);
                            (id, status)
                        })
                        .boxed(),
                ),
                Err(_) => {
                    completed.insert(id);
                }
            }
        }

        let mut receiver_thread_ids: Vec<_> = selected.iter().copied().collect();
        receiver_thread_ids.sort_by_key(|id| id.to_string());
        let receiver_agents: Vec<_> = receiver_thread_ids
            .iter()
            .map(|&thread_id| {
                let metadata = session
                    .services
                    .local_agent_runtime
                    .ensure_agent_known(thread_id)
                    .ok();
                codex_protocol::protocol::CollabAgentRef {
                    thread_id,
                    agent_nickname: metadata
                        .as_ref()
                        .and_then(|agent| agent.agent_nickname.clone()),
                    agent_role: metadata.and_then(|agent| agent.agent_role),
                }
            })
            .collect();
        session
            .emit_turn_item_started(
                &turn,
                &TurnItem::CollabAgentToolCall(CollabAgentToolCallItem {
                    id: call_id.clone(),
                    tool: CollabAgentTool::Wait,
                    status: CollabAgentToolCallStatus::InProgress,
                    sender_thread_id: session.thread_id,
                    receiver_thread_ids: receiver_thread_ids.clone(),
                    receiver_agents: receiver_agents.clone(),
                    prompt: None,
                    model: None,
                    reasoning_effort: None,
                    agents_states: Default::default(),
                }),
            )
            .await;

        session.services.local_agent_runtime.record_feature_use(
            session.thread_id,
            call_id.clone(),
            codex_protocol::feature_use::FeatureUseEvent::WaitStarted {
                all: matches!(args.wait_mode, WaitMode::All),
                progress: args.yield_time_ms.is_some(),
            },
        );
        let wait_started = Instant::now();
        let result = {
            // The losing collection future is dropped before publishing the native
            // outcome. Its request token cancels callbacks and private BTW tasks.
            let progress = collect_progress(
                &session,
                &call_id,
                progress_actors,
                args.yield_time_ms,
                cancellation_token.child_token(),
            );
            tokio::pin!(progress);
            loop {
                let pending = pending_wait_activity(&session, &turn.sub_id).await;
                completed.extend(pending.results.intersection(&selected).copied());
                if cancellation_token.is_cancelled() || pending.steered {
                    break WaitAgentResult::new("Wait interrupted by new input.", &completed);
                }
                if pending.message || outcomes_ready(args.wait_mode, &selected, &completed) {
                    break WaitAgentResult::new("Wait completed.", &completed);
                }
                tokio::select! {
                    biased;
                    _ = cancellation_token.cancelled() => {
                        break WaitAgentResult::new("Wait interrupted by new input.", &completed);
                    }
                    activity = activity_rx.changed() => {
                        if activity.is_err() {
                            break WaitAgentResult::new("Wait input source closed.", &completed);
                        }
                        let activity = *activity_rx.borrow_and_update();
                        if activity == InputQueueActivity::Steer {
                            break WaitAgentResult::new("Wait interrupted by new input.", &completed);
                        }
                        // An untyped native notification still has ordinary wake authority.
                        let pending = pending_wait_activity(&session, &turn.sub_id).await;
                        if pending.message || pending.results.is_empty() {
                            break WaitAgentResult::new("Wait completed.", &completed);
                        }
                    }
                    Some((id, status)) = statuses.next(), if !statuses.is_empty() => {
                        if !matches!(status, AgentStatus::PendingInit | AgentStatus::Running) {
                            completed.insert(id);
                        }
                    }
                    output = &mut progress => {
                        let output = output.map_err(|error| FunctionCallError::RespondToModel(
                            format!("Progress collection unavailable: {error}"),
                        ))?;
                        let mut result = WaitAgentResult::new("Changed-agent progress.", &completed);
                        result.progress = output.text.clone();
                        result.progress_output = Some(output);
                        break result;
                    }
                }
            }
        };
        turn.session_telemetry.record_duration(
            "codex.multi_agent.wait.duration_ms",
            wait_started.elapsed(),
            &[(
                "outcome",
                if result.message == "Wait completed." {
                    "mailbox"
                } else if result.progress_output.is_some() {
                    "progress"
                } else {
                    "steered"
                },
            )],
        );

        if let Some(output) = &result.progress_output
            && !output.delivered_intervals.is_empty()
            && let crate::tools::context::ToolCallSource::CodeMode { cell_id, .. } = &source
        {
            let context_id = session.current_window().await.2.to_string();
            session
                .services
                .executed_tool_calls
                .record_capture_candidate(
                    &codex_code_mode::CellId::new(cell_id.clone()),
                    crate::tools::context::CaptureDeliveryCandidate {
                        rendered_text: output.text.clone(),
                        receipt: codex_history::CaptureDeliveryReceipt {
                            operation_id: call_id.clone(),
                            context_id,
                            intervals: output.delivered_intervals.clone(),
                        },
                    },
                );
        }
        session
            .emit_turn_item_completed(
                &turn,
                TurnItem::CollabAgentToolCall(CollabAgentToolCallItem {
                    id: call_id,
                    tool: CollabAgentTool::Wait,
                    status: CollabAgentToolCallStatus::Completed,
                    sender_thread_id: session.thread_id,
                    receiver_thread_ids,
                    receiver_agents,
                    prompt: None,
                    model: None,
                    reasoning_effort: None,
                    agents_states: HashMap::new(),
                }),
            )
            .await;

        Ok(boxed_tool_output(result))
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    #[serde(default)]
    wait_mode: WaitMode,
    yield_time_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WaitMode {
    #[default]
    Any,
    All,
}

fn outcomes_ready(
    mode: WaitMode,
    selected: &HashSet<ThreadId>,
    completed: &HashSet<ThreadId>,
) -> bool {
    !completed.is_empty()
        && match mode {
            WaitMode::Any => true,
            WaitMode::All => selected.is_subset(completed),
        }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct WaitAgentResult {
    pub(crate) message: String,
    pub(crate) completed_agents: Vec<ThreadId>,
    pub(crate) progress: String,
    /// Candidate delivery only. The outer native result owner acknowledges it.
    #[serde(skip)]
    pub(crate) progress_output: Option<ProgressOutputResponse>,
}

impl WaitAgentResult {
    fn new(message: &str, completed: &HashSet<ThreadId>) -> Self {
        let mut completed_agents: Vec<_> = completed.iter().copied().collect();
        completed_agents.sort_by_key(ToString::to_string);
        Self {
            message: message.to_string(),
            completed_agents,
            progress: String::new(),
            progress_output: None,
        }
    }
}

async fn collect_progress(
    session: &Session,
    operation_id: &str,
    selected: Vec<ProgressActor>,
    yield_time_ms: Option<i64>,
    cancellation: CancellationToken,
) -> anyhow::Result<ProgressOutputResponse> {
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let Some(yield_time_ms) = yield_time_ms else {
        return std::future::pending().await;
    };
    loop {
        tokio::time::sleep(Duration::from_millis(yield_time_ms as u64)).await;
        let Some(frozen) = session
            .select_progress_ranges(operation_id, selected.clone(), cancellation.clone())
            .await?
        else {
            continue;
        };
        // Empty intervals never spend an auxiliary inference or wake the parent.
        let changed: Vec<_> = frozen
            .agents
            .iter()
            .filter(|agent| !agent.intervals.is_empty())
            .collect();
        if changed.is_empty() {
            continue;
        }
        let agents = futures::future::join_all(changed.into_iter().map(|agent| {
            let cancellation = cancellation.child_token();
            async move {
                let question = format!(
                    "Summarize substantive progress for these retained change intervals: {}",
                    serde_json::to_string(&agent.intervals)
                        .expect("capture interval metadata is serializable"),
                );
                let summary = match session
                    .services
                    .local_agent_runtime
                    .answer_recipient_progress_question(
                        agent.actor.thread_id,
                        question,
                        cancellation,
                    )
                    .await
                {
                    Ok(text) => SummaryResult::Available { text },
                    Err(error) => SummaryResult::Unavailable {
                        reason: error.to_string(),
                    },
                };
                PreparedProgressAgent {
                    actor: agent.actor.clone(),
                    intervals: agent.intervals.clone(),
                    summary,
                }
            }
        }))
        .await;
        return session
            .prepare_progress_output(operation_id, frozen, agents, cancellation.clone())
            .await;
    }
}

impl ToolOutput for WaitAgentResult {
    fn log_output(&self) -> String {
        tool_output_json_text(self, "wait_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, /*success*/ None, "wait_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "wait_agent")
    }
}
