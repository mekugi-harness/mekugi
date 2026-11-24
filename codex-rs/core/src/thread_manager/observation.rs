//! Read-only inspection of retained evidence. No writer acquisition or restore.
use super::view::{build_thread_from_snapshot, thread_from_stored_thread};
use super::*;
use codex_app_server_protocol::Thread;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::AgentStatus;

impl ThreadManager {
    async fn registered_agent_runtime(
        &self,
        target: ThreadId,
    ) -> CodexResult<crate::agent::control::LocalAgentRuntime> {
        let threads = self.state.threads.read().await;
        threads
            .values()
            .map(|thread| &thread.session.services.local_agent_runtime)
            .find(|runtime| runtime.registered_root(target).is_some())
            .cloned()
            .ok_or(CodexErr::ThreadNotFound(target))
    }

    /// Passive detail read through native loaded/stored lookup.
    pub async fn read_thread_details(&self, target: ThreadId) -> CodexResult<Thread> {
        self.state.read_thread_view(target, true).await
    }

    /// Native spawn discovery includes the root and never restores an unloaded thread.
    pub async fn read_thread_subtree(&self, root: ThreadId) -> CodexResult<Vec<Thread>> {
        let mut threads = vec![self.state.read_thread_view(root, false).await?];
        for target in self.list_agent_subtree_thread_ids(root).await? {
            if target == root {
                continue;
            }
            match self.state.read_thread_view(target, false).await {
                Ok(thread) => threads.push(thread),
                // The deletion edge is durable before the child is readable.
                Err(err) if matches!(err.details(), CodexErrorDetails::ThreadNotFound(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(threads)
    }

    pub async fn populate_thread_observations(
        &self,
        target: ThreadId,
        thread: &mut Thread,
    ) -> CodexResult<()> {
        self.state
            .populate_thread_details(target, thread, false)
            .await
    }

    pub async fn end_agent(&self, root: ThreadId, target: ThreadId) -> CodexResult<Vec<ThreadId>> {
        let runtime = self.registered_agent_runtime(target).await?;
        if target == root || runtime.registered_root(target) != Some(root) {
            return Err(CodexErr::InvalidRequest(
                "agent end requires a descendant of the owning root".into(),
            ));
        }
        runtime
            .control(root.into())
            .shutdown_agent_tree_with_ids(target)
            .await
            .map(|(_, ended)| ended)
    }

    /// Destination API for an already-authenticated question. Never enrolls its sender
    /// or restores the target. Ordinary sources are frozen under recipient authority.
    pub async fn answer_agent_side_question(
        &self,
        target: ThreadId,
        question: String,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> CodexResult<String> {
        let thread = self.get_thread(target).await?;
        let runtime = &thread.session.services.local_agent_runtime;
        let question = runtime
            .acquire_recipient_message(target, &question, &cancellation)
            .await?;
        runtime
            .answer_recipient_side_question(target, question, None, cancellation)
            .await
    }
}

impl ThreadManager {
    /// Resolve only an already loaded target, without acquiring execution or restoring it.
    pub async fn loaded_agent_root(&self, target: ThreadId) -> CodexResult<ThreadId> {
        self.get_thread(target).await?;
        self.state.agent_root(target).await
    }

    pub async fn receive_foreign_agent_operation(
        &self,
        request: codex_app_server_protocol::AgentForeignRequest,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> CodexResult<codex_app_server_protocol::AgentForeignResponse> {
        use crate::agent_communication::{AgentCommunicationContext, AgentCommunicationKind};
        use codex_app_server_protocol::{
            AgentForeignOperation as Operation, AgentForeignResponse as Response,
        };
        use codex_protocol::protocol::InterAgentCommunication;
        let target = request.target_thread_id;
        let root = self.loaded_agent_root(target).await?;
        let thread = self.get_thread(target).await?;
        let runtime = &thread.session.services.local_agent_runtime;
        if root == request.source.root_thread_id
            || runtime.registered_root(request.source.actor_id).is_some()
        {
            return Err(CodexErr::InvalidRequest(
                "same-tree operations require the local agent owner".into(),
            ));
        }
        if cancellation.is_cancelled() {
            return Err(CodexErr::TurnAborted);
        }
        match request.operation {
            Operation::Message { message } => {
                if message.trim().is_empty() {
                    return Err(CodexErr::InvalidRequest("empty foreign message".into()));
                }
                let message = runtime
                    .acquire_recipient_message(target, &message, &cancellation)
                    .await?;
                let recipient =
                    runtime
                        .ensure_agent_known(target)?
                        .agent_path
                        .ok_or_else(|| {
                            CodexErr::InvalidRequest("recipient has no agent path".into())
                        })?;
                // Paths are tree-local. Preserve the authenticated foreign identity in the
                // delivered content as well as in native delivery diagnostics.
                let attribution = serde_json::to_string(&request.source)
                    .map_err(|error| CodexErr::Fatal(error.to_string()))?;
                let communication = InterAgentCommunication::new(
                    request.source.agent_path,
                    recipient,
                    Vec::new(),
                    format!("Foreign sender: {attribution}\n{message}"),
                    false,
                );
                let delivery_id = runtime
                    .control(root.into())
                    .send_inter_agent_communication(
                        target,
                        communication,
                        AgentCommunicationContext::new(
                            AgentCommunicationKind::Message,
                            request.source.actor_id,
                        ),
                        crate::TurnStartOptions::default(),
                    )
                    .await?;
                Ok(Response::Message {
                    target_thread_id: target,
                    delivery_id,
                    accepted: true,
                })
            }
            Operation::SideQuestion { question } => {
                if question.trim().is_empty() {
                    return Err(CodexErr::InvalidRequest("empty foreign question".into()));
                }
                let answer = self
                    .answer_agent_side_question(target, question, cancellation)
                    .await?;
                Ok(Response::SideQuestion {
                    target_thread_id: target,
                    answer,
                })
            }
            Operation::Inspect => Ok(Response::Inspect {
                thread: serde_json::to_value(self.read_thread_details(target).await?)
                    .map_err(|error| CodexErr::Fatal(error.to_string()))?,
            }),
            Operation::List { path_prefix } => {
                let mut threads = self.read_thread_subtree(root).await?;
                if let Some(prefix) = path_prefix {
                    threads.retain(|thread| {
                        let source: codex_protocol::protocol::SessionSource =
                            thread.source.clone().into();
                        source
                            .get_agent_path()
                            .as_ref()
                            .map(ToString::to_string)
                            .as_deref()
                            .is_some_and(|path| {
                                path == prefix.as_str()
                                    || path
                                        .strip_prefix(prefix.as_str())
                                        .is_some_and(|suffix| suffix.starts_with('/'))
                            })
                    });
                }
                Ok(Response::List {
                    threads: serde_json::to_value(threads)
                        .map_err(|error| CodexErr::Fatal(error.to_string()))?,
                })
            }
        }
    }
}

impl ThreadManagerState {
    async fn agent_root(&self, target: ThreadId) -> CodexResult<ThreadId> {
        let mut current = target;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current) {
                return Err(CodexErr::InvalidRequest(
                    "cyclic thread-spawn ancestry".into(),
                ));
            }
            let parent = match self.get_thread(current).await {
                Ok(thread) => thread.session_source.parent_thread_id(),
                Err(error) if matches!(error.details(), CodexErrorDetails::ThreadNotFound(_)) => {
                    let stored = self
                        .read_stored_thread(ReadThreadParams {
                            thread_id: current,
                            include_archived: true,
                            include_history: false,
                        })
                        .await?;
                    stored.parent_thread_id
                }
                Err(error) => return Err(error),
            };
            match parent {
                Some(parent) => current = parent,
                None => return Ok(current),
            }
        }
    }

    pub(crate) async fn read_thread_view(
        &self,
        target: ThreadId,
        details: bool,
    ) -> CodexResult<Thread> {
        let mut view = match self.get_thread(target).await {
            Ok(thread) => {
                let config = thread.config_snapshot().await;
                build_thread_from_snapshot(
                    target,
                    thread.startup_metadata().session_id.to_string(),
                    thread.multi_agent_version(),
                    &config,
                    thread.rollout_path(),
                )
            }
            Err(error) if matches!(error.details(), CodexErrorDetails::ThreadNotFound(_)) => {
                let stored = self
                    .read_stored_thread(ReadThreadParams {
                        thread_id: target,
                        include_archived: true,
                        include_history: false,
                    })
                    .await?;
                if stored.source.is_internal() {
                    return Err(CodexErr::ThreadNotFound(target));
                }
                let cwd = codex_utils_absolute_path::AbsolutePathBuf::relative_to_current_dir(
                    &stored.cwd,
                )
                .map_err(|error| CodexErr::Fatal(error.to_string()))?;
                thread_from_stored_thread(stored, "", &cwd).0
            }
            Err(error) => return Err(error),
        };
        self.populate_thread_details(target, &mut view, details)
            .await?;
        view.status = match &view.agent_status {
            Some(AgentStatus::Running) => codex_app_server_protocol::ThreadStatus::Active {
                active_flags: Vec::new(),
            },
            Some(AgentStatus::Errored(_)) => codex_app_server_protocol::ThreadStatus::SystemError,
            None | Some(AgentStatus::Shutdown | AgentStatus::NotFound) => {
                codex_app_server_protocol::ThreadStatus::NotLoaded
            }
            _ => codex_app_server_protocol::ThreadStatus::Idle,
        };
        Ok(view)
    }

    async fn populate_thread_details(
        &self,
        target: ThreadId,
        view: &mut Thread,
        include_history: bool,
    ) -> CodexResult<()> {
        let loaded = self.get_thread(target).await.ok();
        let history = if include_history && loaded.is_none() {
            match self
                .thread_store
                .load_history(LoadThreadHistoryParams {
                    thread_id: target,
                    include_archived: true,
                })
                .await
            {
                Ok(history) => Some(history),
                Err(
                    ThreadStoreError::Unsupported { .. } | ThreadStoreError::ThreadNotFound { .. },
                ) => None,
                Err(error) => {
                    return Err(CodexErr::Fatal(format!(
                        "failed to inspect retained thread {target}: {error}"
                    )));
                }
            }
        } else {
            None
        };
        if let Some(history) = history {
            let (settings, metrics) = observe_history(target, &history.items);
            view.context_tokens = metrics
                .context_tokens
                .and_then(|value| value.try_into().ok());
            view.context_window = metrics
                .context_window
                .and_then(|value| value.try_into().ok());
            if let Some(settings) = settings {
                view.model = Some(settings.model);
                view.reasoning_effort = settings.reasoning_effort;
                view.service_tier = settings.service_tier;
                view.cwd = settings.cwd.into();
            }
        }
        if let Some(thread) = loaded {
            let config = thread.config_snapshot().await;
            view.cwd = config.cwd().clone().into();
            view.model = Some(config.model);
            view.service_tier = config.service_tier;
            view.reasoning_effort = config.reasoning_effort;
            let active_settings = {
                let active = thread.session.active_turn.lock().await;
                active
                    .as_ref()
                    .and_then(|turn| turn.task.as_ref())
                    .map(|task| task.turn_context.capture_current_model_settings())
            };
            let context_window = if let Some((model, effort)) = active_settings {
                view.model = Some(model.slug.clone());
                view.reasoning_effort = effort;
                model.usable_context_window()
            } else {
                let model = thread.selected_model_info().await;
                view.reasoning_effort = view
                    .reasoning_effort
                    .take()
                    .or_else(|| model.default_reasoning_level.clone());
                model.usable_context_window()
            };
            view.context_window = context_window.and_then(|value| value.try_into().ok());
            if let Some(usage) = thread.token_usage_info().await {
                view.context_tokens = usage.last_token_usage.total_tokens.try_into().ok();
                view.context_window = usage
                    .model_context_window
                    .or(context_window)
                    .and_then(|value| value.try_into().ok());
            }
            view.completed_compactions = thread.session.completed_compactions().await;
            view.agent_status = Some(match thread.agent_status().await {
                AgentStatus::Completed(_) => AgentStatus::Completed(None),
                status => status,
            });
        } else {
            view.completed_compactions = self
                .thread_store
                .completed_compactions(target)
                .await
                .map_err(|error| CodexErr::Fatal(error.to_string()))?;
        }
        Ok(())
    }
}

fn observe_history(
    thread_id: ThreadId,
    history: &[codex_history::RolloutItem],
) -> (
    Option<codex_protocol::protocol::ThreadSettingsSnapshot>,
    codex_protocol::agent_metrics::AgentMetrics,
) {
    use codex_history::RolloutItem;
    use codex_protocol::protocol::EventMsg;
    let mut settings = None;
    let mut metrics = codex_protocol::agent_metrics::AgentMetrics::default();
    let mut owner = None;
    for item in history {
        match item {
            RolloutItem::SessionMeta(meta) => {
                owner = Some(meta.meta.id);
            }
            RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event)) => {
                if let Some(id) = event.thread_id {
                    owner = Some(id);
                }
                if owner == Some(thread_id) {
                    settings = Some(event.thread_settings.clone());
                }
            }
            RolloutItem::EventMsg(EventMsg::TokenCount(event)) if owner == Some(thread_id) => {
                if let Some(usage) = &event.info {
                    metrics.context_tokens = Some(usage.last_token_usage.total_tokens);
                    metrics.context_window = usage.model_context_window;
                }
            }
            _ => {}
        }
    }
    (settings, metrics)
}
