//! Read-only inspection of retained evidence. No writer acquisition or restore.
use super::view::{build_thread_from_snapshot, thread_from_stored_thread};
use super::*;
use codex_app_server_protocol::Thread;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::AgentStatus;
use futures::StreamExt;

/// A live foreign target. Resolving it grants only messages, private questions
/// and passive inspection, never enrollment, restore or lifecycle control.
pub(crate) struct ForeignAgent {
    target: Arc<CodexThread>,
    actor: ThreadId,
    actor_path: codex_protocol::AgentPath,
    origin_root: ThreadId,
    cancellation: tokio_util::sync::CancellationToken,
}

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
        let threads = self
            .state
            .read_thread_subtree(root, &self.agent_control().runtime)
            .await?;
        // Keeping the futures in this request cancels pending reads with it. Bound
        // concurrency because a retained history can be large.
        let reads = threads.into_iter().map(|mut thread| async move {
            if let Ok(thread_id) = ThreadId::from_string(&thread.id) {
                self.populate_retained_thread_usage(thread_id, &mut thread)
                    .await;
            }
            thread
        });
        Ok(futures::stream::iter(reads).buffered(4).collect().await)
    }

    /// Populates usage for a detached thread without resuming it.
    /// Unavailable accounting does not prevent reading the thread.
    pub async fn populate_retained_thread_usage(&self, target: ThreadId, thread: &mut Thread) {
        if thread.path.is_none()
            || thread.token_usage.is_some()
            || self.state.get_thread(target).await.is_ok()
        {
            return;
        }
        match self
            .state
            .thread_store
            .load_history(LoadThreadHistoryParams {
                thread_id: target,
                include_archived: true,
            })
            .await
        {
            Ok(history) => Self::apply_retained_thread_usage(thread, &history),
            Err(error) => tracing::debug!("retained usage for {target} is unavailable: {error}"),
        }
    }

    /// Reuse a history already admitted by a read rather than loading it again.
    pub fn apply_retained_thread_usage(
        thread: &mut Thread,
        history: &codex_thread_store::StoredThreadHistory,
    ) {
        apply_retained_usage(thread, history.thread_id, &history.items);
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
}

impl crate::agent::control::LocalAgentRuntime {
    pub(crate) async fn foreign_agent(
        &self,
        actor: ThreadId,
        target: ThreadId,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> CodexResult<ForeignAgent> {
        let manager = self.upgrade()?;
        let source = manager.get_thread(actor).await?;
        let origin_root = self
            .registered_root(actor)
            .ok_or(CodexErr::ThreadNotFound(actor))?;
        if source.session_source.is_internal()
            || source.has_shutdown_status()
            || source.session.services.local_agent_runtime.tree_id() != self.tree_id()
        {
            return Err(CodexErr::ThreadNotFound(actor));
        }
        let target = manager.get_thread(target).await?;
        if target.session_source.is_internal() || target.has_shutdown_status() {
            return Err(CodexErr::ThreadNotFound(target.session.thread_id));
        }
        if target.agent_root_id() == origin_root {
            return Err(CodexErr::InvalidRequest(
                "same-tree operations require the local agent owner".into(),
            ));
        }
        let runtime = &target.session.services.local_agent_runtime;
        if cancellation.is_cancelled() || runtime.shutdown.is_cancelled() {
            return Err(CodexErr::TurnAborted);
        }
        runtime.register_session_root(
            target.session.thread_id,
            target.session_source.parent_thread_id(),
        );
        runtime.ensure_agent_known(target.session.thread_id)?;
        let actor_path = self
            .ensure_agent_known(actor)?
            .agent_path
            .ok_or(CodexErr::ThreadNotFound(actor))?;
        Ok(ForeignAgent {
            target,
            actor,
            actor_path,
            origin_root,
            cancellation,
        })
    }
}

impl ForeignAgent {
    pub(crate) fn id(&self) -> ThreadId {
        self.target.session.thread_id
    }

    async fn while_live<T>(
        &self,
        operation: impl Future<Output = CodexResult<T>>,
    ) -> CodexResult<T> {
        let shutdown = &self.target.session.services.local_agent_runtime.shutdown;
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(CodexErr::TurnAborted),
            _ = shutdown.cancelled() => Err(CodexErr::TurnAborted),
            result = operation => result,
        }
    }

    pub(crate) async fn message(&self, message: &str, turn: &str, call: &str) -> CodexResult<()> {
        use crate::agent_communication::{AgentCommunicationContext, AgentCommunicationKind};
        use codex_protocol::protocol::InterAgentCommunication;
        let id = self.id();
        let runtime = &self.target.session.services.local_agent_runtime;
        self.while_live(async {
            let message = runtime.acquire_recipient_message(id, message, &self.cancellation).await?;
            let recipient = runtime.ensure_agent_known(id)?.agent_path.ok_or(CodexErr::ThreadNotFound(id))?;
            let attribution = serde_json::json!({"rootThreadId":self.origin_root,"actorId":self.actor,
                "agentPath":self.actor_path,"turnId":turn,"callId":call});
            let communication = InterAgentCommunication::new(
                self.actor_path.clone(), recipient, Vec::new(), format!("Foreign sender: {attribution}\n{message}"), false,
            );
            runtime.control(self.target.session.session_id()).send_inter_agent_communication(
                id, communication, AgentCommunicationContext::new(AgentCommunicationKind::Message, self.actor),
                crate::TurnStartOptions::default(),
            ).await.map(|_| ())
        }).await
    }

    pub(crate) async fn answer(&self, question: &str) -> CodexResult<String> {
        let runtime = &self.target.session.services.local_agent_runtime;
        self.while_live(async {
            let question = runtime
                .acquire_recipient_message(self.id(), question, &self.cancellation)
                .await?;
            runtime
                .answer_recipient_side_question(
                    self.id(),
                    question,
                    None,
                    self.cancellation.clone(),
                )
                .await
        })
        .await
    }

    pub(crate) async fn inspect(&self) -> CodexResult<Thread> {
        let runtime = &self.target.session.services.local_agent_runtime;
        self.while_live(runtime.read_thread_details(self.id()))
            .await
    }

    pub(crate) async fn list(&self) -> CodexResult<Vec<Thread>> {
        let runtime = &self.target.session.services.local_agent_runtime;
        self.while_live(
            runtime
                .upgrade()?
                .read_thread_subtree(self.target.agent_root_id(), runtime),
        )
        .await
    }
}

impl ThreadManagerState {
    pub(super) async fn list_agent_subtree_thread_ids(
        &self,
        thread_id: ThreadId,
        runtime: &crate::agent::control::LocalAgentRuntime,
    ) -> CodexResult<Vec<ThreadId>> {
        let mut subtree_thread_ids = Vec::new();
        let mut seen_thread_ids = HashSet::new();
        subtree_thread_ids.push(thread_id);
        seen_thread_ids.insert(thread_id);

        if let Some(agent_graph_store) = self.agent_graph_store() {
            for descendant_id in agent_graph_store
                .list_thread_spawn_descendants(thread_id, /*status_filter*/ None)
                .await
                .map_err(|err| {
                    CodexErr::Fatal(format!("failed to load thread-spawn descendants: {err}"))
                })?
            {
                if seen_thread_ids.insert(descendant_id) {
                    subtree_thread_ids.push(descendant_id);
                }
            }
        }

        for descendant_id in runtime
            .list_live_agent_subtree_thread_ids(thread_id)
            .await?
        {
            if seen_thread_ids.insert(descendant_id) {
                subtree_thread_ids.push(descendant_id);
            }
        }

        Ok(subtree_thread_ids)
    }

    async fn read_thread_subtree(
        &self,
        root: ThreadId,
        runtime: &crate::agent::control::LocalAgentRuntime,
    ) -> CodexResult<Vec<Thread>> {
        let mut threads = vec![self.read_thread_view(root, false).await?];
        for target in self.list_agent_subtree_thread_ids(root, runtime).await? {
            if target == root {
                continue;
            }
            match self.read_thread_view(target, false).await {
                Ok(thread) => threads.push(thread),
                // The spawn edge is durable before the child is readable.
                Err(err) if matches!(err.details(), CodexErrorDetails::ThreadNotFound(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(threads)
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
            if let Some(settings) = apply_retained_usage(view, target, &history.items) {
                view.model = Some(settings.model.clone());
                view.reasoning_effort.clone_from(&settings.reasoning_effort);
                view.service_tier.clone_from(&settings.service_tier);
                view.cwd = settings.cwd.clone().into();
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
            view.token_usage = thread.observed_token_usage().await;
            view.extra = thread.observed_thread_extra().await;
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

fn apply_retained_usage<'a>(
    view: &mut Thread,
    target: ThreadId,
    items: &'a [codex_history::RolloutItem],
) -> Option<&'a codex_protocol::protocol::ThreadSettingsSnapshot> {
    let observation = crate::session::usage_metrics::ThreadUsageMetrics::restore(target, items);
    if let Some(info) = observation.token_info {
        view.context_tokens = info.last_token_usage.total_tokens.try_into().ok();
        view.context_window = info
            .model_context_window
            .and_then(|window| window.try_into().ok());
    }
    view.token_usage = observation.usage.snapshot(observation.token_info.cloned());
    if !observation.skills.is_empty() {
        view.extra = Some(codex_app_server_protocol::ThreadExtra {
            skills: observation.skills.into_iter().collect(),
            background_terminal_count: None,
        });
    }
    observation.settings
}
