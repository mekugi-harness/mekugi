use crate::unified_exec::output::CapturedStream;
use crate::unified_exec::output::slice_next_page;
use crate::unified_exec::output::unified_exec_page_byte_budget;
use crate::unified_exec::retained::RetainedExecMeta;
use crate::unified_exec::retained::read_retained_window;
use rand::Rng;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::Mutex;
use tokio::sync::watch;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use super::oneshot::Completion;
use super::process::OutputDrainPolicy;

use crate::codex_thread::BackgroundTerminalInfo;
use crate::exec_env::CODEX_PERMISSION_PROFILE_ENV_VAR;
use crate::exec_env::CODEX_THREAD_ID_ENV_VAR;
use crate::exec_env::CODEX_VERSION_ENV_VAR;
use crate::exec_env::SESSION_TMP_ENV_VAR;
use crate::exec_env::create_env;
use crate::exec_env::inject_apply_patch_env;
use crate::exec_env::inject_permission_profile_env;
use crate::exec_env::inject_session_env;
use crate::exec_env::set_tool_call_id_env_var;
use crate::guardian::GuardianReviewContext;
use crate::plugins::metrics::finish_and_track_measurements;
use crate::sandboxing::ExecRequest;
use crate::sandboxing::ExecServerEnvConfig;
use crate::tools::ApprovalContext;
use crate::tools::context::ExecCommandToolOutput;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::events::ToolEventStage;
use crate::tools::network_approval::DeferredNetworkApproval;
use crate::tools::network_approval::finish_deferred_network_approval;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::runtimes::is_managed_proxy_env_var;
use crate::tools::runtimes::unified_exec::UnifiedExecAttempt;
use crate::tools::runtimes::unified_exec::UnifiedExecRequest as UnifiedExecToolRequest;
use crate::tools::runtimes::unified_exec::UnifiedExecRuntime;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::unified_exec::ExecCommandRequest;
use crate::unified_exec::MAX_UNIFIED_EXEC_PROCESSES;
use crate::unified_exec::MAX_YIELD_TIME_MS;
use crate::unified_exec::MIN_EMPTY_YIELD_TIME_MS;
use crate::unified_exec::MIN_YIELD_TIME_MS;
use crate::unified_exec::ProcessEntry;
use crate::unified_exec::ProcessStore;
use crate::unified_exec::RetainedEntry;
use crate::unified_exec::TerminalSession;
use crate::unified_exec::TerminalTarget;
use crate::unified_exec::UnifiedExecContext;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcessManager;
use crate::unified_exec::WriteStdinInteractionEvent;
use crate::unified_exec::WriteStdinRequest;
use crate::unified_exec::async_watcher::emit_exec_end_for_unified_exec;
use crate::unified_exec::async_watcher::emit_failed_exec_end_for_unified_exec;
use crate::unified_exec::async_watcher::spawn_exit_watcher;
use crate::unified_exec::async_watcher::start_streaming_output;
use crate::unified_exec::clamp_yield_time;
use crate::unified_exec::generate_chunk_id;
use crate::unified_exec::process::OutputBuffers;
use crate::unified_exec::process::OutputHandles;
use crate::unified_exec::process::UnifiedExecProcess;
use crate::unified_exec::shell_snapshot::shell_snapshot_request;
use crate::unified_exec::take_plugin_metrics_sidecar;
use crate::unified_exec::trace_id;
use codex_core_plugins::PLUGIN_METRICS_OUTPUT_ENV_VAR;
use codex_core_plugins::PluginCommandAttribution;
use codex_core_plugins::PluginMetricsSidecar;
use codex_core_plugins::strip_output_env;
use codex_network_proxy::NetworkPolicyDecider;
use codex_network_proxy::NetworkProxy;
use codex_protocol::config_types::ShellEnvironmentPolicy;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::SandboxErr;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ExecCommandSource;
use codex_protocol::protocol::TerminalInteractionEvent;
use codex_protocol::shell_environment::is_non_inheritable_tool_env_var;
use codex_tools::ToolName;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::approx_tokens_from_byte_count;
use codex_utils_path_uri::PathUri;

const UNIFIED_EXEC_ENV: [(&str, &str); 10] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("LANG", "C.UTF-8"),
    ("LC_CTYPE", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("CODEX_CI", "1"),
];

fn approx_tokens_from_u64(bytes: u64) -> usize {
    let clamped = usize::try_from(bytes).unwrap_or(usize::MAX);
    usize::try_from(approx_tokens_from_byte_count(clamped)).unwrap_or(usize::MAX)
}
const NETWORK_ACCESS_DENIED_MESSAGE: &str =
    "Network access was denied by the Codex sandbox network proxy.";
const LATE_NETWORK_DENIAL_GRACE_PERIOD: Duration = Duration::from_millis(100);
const MAX_STDIN_APPROVAL_BYTES: usize = 8_000;
const INTERRUPT: &str = "\u{3}";

/// Test-only override for deterministic unified exec process IDs.
///
/// In production builds this value should remain at its default (`false`) and
/// must not be toggled.
static FORCE_DETERMINISTIC_PROCESS_IDS: AtomicBool = AtomicBool::new(false);

pub(super) fn set_deterministic_process_ids_for_tests(enabled: bool) {
    FORCE_DETERMINISTIC_PROCESS_IDS.store(enabled, Ordering::Relaxed);
}

fn deterministic_process_ids_forced_for_tests() -> bool {
    FORCE_DETERMINISTIC_PROCESS_IDS.load(Ordering::Relaxed)
}

pub(super) fn should_use_deterministic_process_ids() -> bool {
    cfg!(test) || deterministic_process_ids_forced_for_tests()
}

pub(super) fn apply_unified_exec_env(mut env: HashMap<String, String>) -> HashMap<String, String> {
    for (key, value) in UNIFIED_EXEC_ENV {
        env.insert(key.to_string(), value.to_string());
    }
    env
}

pub(crate) fn exec_env_policy_from_shell_policy(
    policy: &ShellEnvironmentPolicy,
) -> codex_exec_server::ExecEnvPolicy {
    let mut exclude = policy
        .exclude
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>();
    exclude.extend([
        CODEX_PERMISSION_PROFILE_ENV_VAR.to_string(),
        CODEX_VERSION_ENV_VAR.to_string(),
        codex_apply_patch::CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR.to_string(),
        PLUGIN_METRICS_OUTPUT_ENV_VAR.to_string(),
    ]);
    let mut r#set = policy.r#set.clone();
    r#set.retain(|key, _| {
        ![
            CODEX_PERMISSION_PROFILE_ENV_VAR,
            CODEX_VERSION_ENV_VAR,
            codex_apply_patch::CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR,
            PLUGIN_METRICS_OUTPUT_ENV_VAR,
        ]
        .iter()
        .any(|runtime_key| key.eq_ignore_ascii_case(runtime_key))
            && !is_non_inheritable_tool_env_var(key)
    });
    codex_exec_server::ExecEnvPolicy {
        inherit: policy.inherit.clone(),
        ignore_default_excludes: policy.ignore_default_excludes,
        exclude,
        r#set,
        include_only: policy
            .include_only
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
    }
}

fn env_overlay_for_exec_server(
    request_env: &HashMap<String, String>,
    local_policy_env: &HashMap<String, String>,
) -> HashMap<String, String> {
    request_env
        .iter()
        .filter(|(key, value)| {
            !is_non_inheritable_tool_env_var(key)
                && (matches!(
                    key.as_str(),
                    CODEX_PERMISSION_PROFILE_ENV_VAR
                        | CODEX_VERSION_ENV_VAR
                        | SESSION_TMP_ENV_VAR
                        | codex_apply_patch::CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR
                ) || local_policy_env.get(*key) != Some(*value))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn exec_server_env_for_request(
    request: &ExecRequest,
) -> (
    Option<codex_exec_server::ExecEnvPolicy>,
    HashMap<String, String>,
) {
    if let Some(exec_server_env_config) = &request.exec_server_env_config {
        let mut env =
            env_overlay_for_exec_server(&request.env, &exec_server_env_config.local_policy_env);
        if request.exec_server_shell_snapshot.is_some()
            && !exec_server_env_config.policy.r#set.contains_key("PATH")
        {
            env.remove("PATH");
        }
        if request.exec_server_managed_network.is_some() {
            for (key, value) in &request.env {
                if is_managed_proxy_env_var(key, value) {
                    env.insert(key.clone(), value.clone());
                }
            }
        }
        (Some(exec_server_env_config.policy.clone()), env)
    } else {
        (None, request.env.clone())
    }
}

fn exec_server_params_for_request(
    process_id: i32,
    request: &ExecRequest,
    tool_ctx: Option<&ToolCtx>,
    windows_sandbox_proxy_settings_mode: codex_sandboxing::WindowsSandboxProxySettingsMode,
    tty: bool,
) -> codex_exec_server::ExecParams {
    let (env_policy, env) = exec_server_env_for_request(request);
    let sandbox = request.exec_server_sandbox.clone().map(|mut sandbox| {
        sandbox.windows_sandbox_proxy_settings_mode = Some(windows_sandbox_proxy_settings_mode);
        sandbox
    });
    // Threads sharing an executor and sandbox retries can reuse a public handle
    // while the executor still retains its previous process.
    let exec_server_process_id = format!("{process_id}-{}", Uuid::new_v4());
    let mut params = codex_exec_server::ExecParams {
        process_id: exec_server_process_id.into(),
        metadata: tool_ctx.map(|ctx| codex_exec_server::ExecMetadata {
            thread_id: Some(ctx.session.thread_id()),
            tool_call_id: Some(ctx.call_id.clone()),
        }),
        argv: request.command.clone(),
        cwd: request.cwd.clone(),
        env_policy,
        shell_snapshot: request.exec_server_shell_snapshot.clone(),
        env,
        tty,
        pipe_stdin: false,
        arg0: request.arg0.clone(),
        sandbox,
        enforce_managed_network: request.exec_server_enforce_managed_network,
        managed_network: request.exec_server_managed_network.clone(),
        network_proxy: request.exec_server_network_proxy.clone(),
    };
    let sandboxed = params.sandbox.is_some();
    // A prepared context uses the local sandbox. A remote proxy uses the executor's OS.
    if let Some(local) = &mut params.managed_network {
        local.transparent_tcp &= sandboxed && cfg!(target_os = "linux");
    }
    if let Some(remote) = &mut params.network_proxy {
        remote.proxy.transparent_tcp &= sandboxed;
    }
    params
}

/// Borrowed process state prepared for a `write_stdin` or poll operation.
struct PreparedProcessHandles {
    process: Arc<UnifiedExecProcess>,
    output: OutputHandles,
    pause_state: Option<watch::Receiver<bool>>,
    session: Option<Arc<crate::session::session::Session>>,
    network_approval: Option<DeferredNetworkApproval>,
    call_id: String,
    environment_id: String,
    hook_command: String,
    tool_cwd: Option<PathUri>,
    process_id: i32,
    tty: bool,
    persisted_bytes: usize,
    persisted: bool,
}

struct RetainedSnapshot {
    bytes: Vec<u8>,
    retained_bytes: usize,
    discarded_bytes: u64,
    read_offset: usize,
    reported_discarded_bytes: u64,
}

struct InitialExecCommandGuard {
    active: Option<Arc<AtomicBool>>,
    metrics_sidecar: Option<PluginMetricsSidecar>,
    collection_finished: CancellationToken,
}

impl InitialExecCommandGuard {
    async fn finish_plugin_metrics(&mut self, context: &UnifiedExecContext, exit_code: i32) {
        let model_context = context.step_context.model_context();
        finish_and_track_measurements(
            self.metrics_sidecar.take(),
            exit_code,
            &context.session,
            &context.step_context.turn,
            &model_context,
            &context.call_id,
        )
        .await;
    }
}

impl Drop for InitialExecCommandGuard {
    fn drop(&mut self) {
        if let Some(active) = self.active.as_ref() {
            active.store(false, Ordering::Release);
        }
        self.collection_finished.cancel();
    }
}

async fn unregister_network_approval_for_entry(entry: &ProcessEntry) {
    if let Some(network_approval) = entry.network_approval.as_ref()
        && let Some(session) = entry.session.upgrade()
    {
        session
            .services
            .network_approval
            .unregister_call(network_approval.registration_id())
            .await;
    }
}

async fn finish_network_approval_after_process_exit_for_entry(
    entry: &ProcessEntry,
) -> Result<(), String> {
    let session = entry.session.upgrade();
    finish_deferred_network_approval_after_process_exit_for_session(
        session.as_ref(),
        entry.network_approval.clone(),
    )
    .await
}

async fn finish_deferred_network_approval_for_session(
    session: Option<&Arc<crate::session::session::Session>>,
    deferred: Option<DeferredNetworkApproval>,
) -> Result<(), String> {
    let Some(session) = session else {
        return Ok(());
    };
    finish_deferred_network_approval(session.as_ref(), deferred)
        .await
        .map_err(network_approval_error_message)
}

fn network_approval_error_message(err: ToolError) -> String {
    match err {
        ToolError::Rejected(message) => message,
        ToolError::Codex(err) => err.to_string(),
    }
}

async fn network_denial_message_for_session(
    session: Option<&Arc<crate::session::session::Session>>,
    deferred: Option<DeferredNetworkApproval>,
) -> String {
    let Some(session) = session else {
        return NETWORK_ACCESS_DENIED_MESSAGE.to_string();
    };
    match finish_deferred_network_approval(session.as_ref(), deferred).await {
        Ok(()) => NETWORK_ACCESS_DENIED_MESSAGE.to_string(),
        Err(err) => network_approval_error_message(err),
    }
}

async fn wait_for_late_network_denial(network_cancelled: &CancellationToken) -> bool {
    tokio::time::timeout(
        LATE_NETWORK_DENIAL_GRACE_PERIOD,
        network_cancelled.cancelled(),
    )
    .await
    .is_ok()
}

async fn finish_deferred_network_approval_after_process_exit_for_session(
    session: Option<&Arc<crate::session::session::Session>>,
    deferred: Option<DeferredNetworkApproval>,
) -> Result<(), String> {
    if let Some(approval) = &deferred {
        let network_cancelled = approval.cancellation_token();
        // Finishing below unregisters the call before we report the outcome.
        tokio::select! {
            biased;
            _ = approval.remote_decisions_drained() => {}
            _ = wait_for_late_network_denial(&network_cancelled) => {}
        }
    }
    finish_deferred_network_approval_for_session(session, deferred).await
}

fn fail_process_with_message(process: &UnifiedExecProcess, message: String) -> UnifiedExecError {
    if let Some(message) = process.failure_message() {
        process.terminate();
        return UnifiedExecError::process_failed(message);
    }

    process.fail_and_terminate(message.clone());
    UnifiedExecError::process_failed(process.failure_message().unwrap_or(message))
}

#[allow(clippy::too_many_arguments)]
async fn emit_failed_initial_exec_end_if_unstored(
    process_stored: bool,
    sandbox_type: Option<codex_protocol::sandbox::SandboxType>,
    context: &UnifiedExecContext,
    request: &ExecCommandRequest,
    cwd: PathUri,
    plugin_attribution: Option<PluginCommandAttribution>,
    output_buffer: Arc<tokio::sync::Mutex<OutputBuffers>>,
    fallback_output: String,
    message: String,
    wall_time: Duration,
) {
    if process_stored {
        return;
    }

    emit_failed_exec_end_for_unified_exec(
        sandbox_type,
        Arc::clone(&context.session),
        Arc::clone(&context.step_context.turn),
        Arc::clone(&context.step_context.settings.model_info),
        context.call_id.clone(),
        request.prepared.resolved.command.clone(),
        cwd,
        Some(request.process_id.to_string()),
        plugin_attribution,
        output_buffer,
        fallback_output,
        message,
        wall_time,
    )
    .await;
}

fn terminate_process_on_network_denial(
    process: Arc<UnifiedExecProcess>,
    session: std::sync::Weak<crate::session::session::Session>,
    deferred: DeferredNetworkApproval,
) -> tokio::task::JoinHandle<()> {
    let network_cancelled = deferred.cancellation_token();
    let process_exited = process.cancellation_token();
    tokio::spawn(async move {
        let denied = tokio::select! {
            _ = network_cancelled.cancelled() => true,
            _ = process_exited.cancelled() => {
                // A yielded process stays registered until its next poll.
                wait_for_late_network_denial(&network_cancelled).await
            }
        };
        if !denied {
            return;
        }
        let session = session.upgrade();
        let message = network_denial_message_for_session(session.as_ref(), Some(deferred)).await;
        process.fail_and_terminate(message);
    })
}

impl UnifiedExecProcessManager {
    pub(crate) async fn allocate_process_id(&self) -> i32 {
        loop {
            let mut store = self.process_store.lock().await;

            let process_id = if should_use_deterministic_process_ids() {
                // test or deterministic mode
                store
                    .reserved_process_ids
                    .iter()
                    .copied()
                    .max()
                    .map(|m| std::cmp::max(m, 999) + 1)
                    .unwrap_or(1000)
            } else {
                // production mode → random
                rand::rng().random_range(1_000..100_000)
            };

            if store.reserved_process_ids.contains(&process_id) {
                continue;
            }

            store.reserved_process_ids.insert(process_id);
            return process_id;
        }
    }

    pub(crate) async fn release_process_id(&self, process_id: i32) {
        self.release_process(process_id, /*expected_process*/ None)
            .await;
    }

    async fn release_process(
        &self,
        process_id: i32,
        expected_process: Option<&Arc<UnifiedExecProcess>>,
    ) {
        let removed = {
            let mut store = self.process_store.lock().await;
            if let Some(expected) = expected_process
                && !store
                    .processes
                    .get(&process_id)
                    .is_some_and(|entry| Arc::ptr_eq(&entry.process, expected))
            {
                return;
            }
            store.remove(process_id)
        };
        if let Some(entry) = removed {
            unregister_network_approval_for_entry(&entry).await;
        }
    }

    #[tracing::instrument(
        name = "unified_exec.exec_command",
        level = "info",
        skip_all,
        fields(
            conversation.id = %context.session.thread_id,
            turn_id = trace_id(&context.step_context.turn.sub_id),
            call_id = trace_id(&context.call_id),
            unified_exec_process_id = request.process_id,
            mode = "resumable",
            outcome = tracing::field::Empty,
        )
    )]
    pub(crate) async fn exec_command(
        &self,
        request: ExecCommandRequest,
        context: &UnifiedExecContext,
    ) -> Result<ExecCommandToolOutput, UnifiedExecError> {
        let result = self
            .exec_command_inner(request, context, /*completion*/ None)
            .await;
        let outcome = match &result {
            Ok(output) if output.process_id.is_some() => "yielded",
            Ok(_) => "exited",
            Err(_) if context.cancellation_token.is_cancelled() => "cancelled",
            Err(_) => "failed",
        };
        tracing::Span::current().record("outcome", outcome);
        result
    }

    pub(super) async fn exec_command_inner(
        &self,
        request: ExecCommandRequest,
        context: &UnifiedExecContext,
        mut completion: Option<&mut Completion<'_>>,
    ) -> Result<ExecCommandToolOutput, UnifiedExecError> {
        let prepared = &request.prepared;
        let command = &prepared.resolved.command;
        let cwd = prepared.cwd.clone();
        let process = self.open_session_with_sandbox(&request, context).await;

        let (attempt, mut deferred_network_approval) = match process {
            Ok((attempt, deferred_network_approval)) => (attempt, deferred_network_approval),
            Err(err) => {
                self.release_process_id(request.process_id).await;
                return Err(err);
            }
        };
        let UnifiedExecAttempt {
            process,
            metrics_sidecar,
            permissions,
        } = attempt;
        let process = Arc::new(process);
        if let Some(completion) = completion.as_ref() {
            let _ = completion.process.set(Arc::clone(&process));
        }
        let network_denial_monitor = deferred_network_approval.as_ref().map(|deferred| {
            terminate_process_on_network_denial(
                Arc::clone(&process),
                Arc::downgrade(&context.session),
                deferred.clone(),
            )
        });

        let output_buffer = Arc::clone(&process.output_handles().output_buffer);
        let model_context = context.step_context.model_context();
        let mut event_ctx = ToolEventCtx::new(
            context.session.as_ref(),
            context.step_context.turn.as_ref(),
            &context.step_context.settings.model_info,
            &context.call_id,
            /*turn_diff_tracker*/ None,
        );
        event_ctx.model_context = Some(&model_context);
        let plugin_attribution = if prepared.turn_environment.environment().is_remote() {
            let file_system = prepared.turn_environment.environment().get_filesystem();
            context
                .step_context
                .turn
                .plugin_attribution_for_executor_command(command, &cwd, file_system.as_ref())
                .await
        } else {
            cwd.to_abs_path().ok().and_then(|cwd| {
                context
                    .step_context
                    .turn
                    .plugin_attribution_for_command(command, &cwd)
            })
        };
        let emitter = ToolEmitter::unified_exec(
            command,
            cwd.clone(),
            ExecCommandSource::UnifiedExecStartup,
            Some(request.process_id.to_string()),
            plugin_attribution.clone(),
        );
        emitter.emit(event_ctx, ToolEventStage::Begin).await;

        // Serialize remote collection with later polls and final transcript publication.
        let _interaction_guard =
            if process.output_handles().drain_policy == OutputDrainPolicy::WaitForOutputClosure {
                Some(process.interaction_lock().lock_owned().await)
            } else {
                None
            };
        start_streaming_output(&process, context);
        let start = Instant::now();
        // Persist live sessions and remote output that remains pollable after an early exit.
        let process_stored = (!process.has_exited() && process.exit_code().is_none())
            || (process.output_handles().drain_policy == OutputDrainPolicy::WaitForOutputClosure
                && !process
                    .output_handles()
                    .output_closed
                    .load(Ordering::Acquire));
        let mut initial_exec_command_guard = InitialExecCommandGuard {
            active: None,
            metrics_sidecar,
            collection_finished: process.output_handles().collection_finished.clone(),
        };
        if process_stored {
            let initial_exec_command_active = Arc::new(AtomicBool::new(true));
            initial_exec_command_guard.active = Some(Arc::clone(&initial_exec_command_active));
            self.store_process(
                Arc::clone(&process),
                context,
                command,
                prepared.hook_command.clone(),
                cwd.clone(),
                prepared.turn_environment.selection.environment_id.clone(),
                permissions,
                plugin_attribution.clone(),
                start,
                request.process_id,
                prepared.tty,
                deferred_network_approval.clone(),
                network_denial_monitor,
                initial_exec_command_guard.metrics_sidecar.take(),
                Arc::clone(&output_buffer),
                Arc::clone(&initial_exec_command_active),
            )
            .await;
        }

        let yield_time_ms = clamp_yield_time(prepared.yield_time_ms);
        let truncation_policy: TruncationPolicy = context
            .step_context
            .settings
            .model_info
            .truncation_policy
            .into();
        let is_internal_unpaged = completion
            .as_ref()
            .is_some_and(|completion| completion.timeout.is_none());
        let page_budget =
            unified_exec_page_byte_budget(prepared.max_output_tokens, truncation_policy);
        // For the initial exec_command call, we both stream output to events
        // (via start_streaming_output above) and wait for output up to the
        // yield/completion deadline before slicing the first page.
        let wait = completion.as_ref().map_or_else(
            || Some(Duration::from_millis(yield_time_ms)),
            |completion| completion.timeout,
        );
        if let Some(wait) = wait {
            let deadline = start.checked_add(wait).ok_or_else(|| {
                UnifiedExecError::process_failed("timeout_ms is too large".into())
            })?;
            Self::wait_for_output_until_deadline(
                process.output_handles(),
                Some(context.session.subscribe_elicitation_pause_state()),
                deadline,
            )
            .await;
        } else {
            // Internal source readers wait for exit and closed pipes without a polling deadline.
            let output = process.output_handles();
            tokio::select! {
                _ = process.wait_for_exit() => {}
                _ = context.cancellation_token.cancelled() => {}
            }
            loop {
                let closed = output.output_closed_notify.notified();
                tokio::pin!(closed);
                closed.as_mut().enable();
                if output.output_closed.load(Ordering::Acquire) {
                    break;
                }
                tokio::select! {
                    _ = &mut closed => {}
                    _ = context.cancellation_token.cancelled() => break,
                }
            }
        }
        if let Some(completion) = completion.as_mut()
            && completion.timeout.is_some()
            && !process.has_exited()
        {
            completion.timed_out = true;
            process.mark_timed_out();
            if let Err(err) = process.terminate_confirmed().await {
                process.fail_and_terminate(err.to_string());
            }
        }
        let wall_time = Instant::now().saturating_duration_since(start);
        let chunk_id = generate_chunk_id();
        if deferred_network_approval
            .as_ref()
            .is_some_and(DeferredNetworkApproval::is_cancelled)
        {
            let fallback_text = output_buffer
                .lock()
                .await
                .retained
                .to_lossy_string_with_discard_notice();
            let message = network_denial_message_for_session(
                Some(&context.session),
                deferred_network_approval.take(),
            )
            .await;
            emit_failed_initial_exec_end_if_unstored(
                process_stored,
                process.sandbox_type(),
                context,
                &request,
                cwd.clone(),
                plugin_attribution.clone(),
                Arc::clone(&output_buffer),
                fallback_text,
                message.clone(),
                wall_time,
            )
            .await;
            self.release_process_id(request.process_id).await;
            return Err(fail_process_with_message(process.as_ref(), message));
        }
        if let Some(message) = process.failure_message() {
            let fallback_text = output_buffer
                .lock()
                .await
                .retained
                .to_lossy_string_with_discard_notice();
            let finish_result = finish_deferred_network_approval_for_session(
                Some(&context.session),
                deferred_network_approval.take(),
            )
            .await;
            emit_failed_initial_exec_end_if_unstored(
                process_stored,
                process.sandbox_type(),
                context,
                &request,
                cwd.clone(),
                plugin_attribution.clone(),
                Arc::clone(&output_buffer),
                fallback_text,
                message.clone(),
                wall_time,
            )
            .await;
            self.release_process_id(request.process_id).await;
            if let Err(message) = finish_result {
                return Err(fail_process_with_message(process.as_ref(), message));
            }
            return Err(UnifiedExecError::process_failed(message));
        }

        let process_id = request.process_id;
        let environment_id = prepared.turn_environment.selection.environment_id.clone();
        let tool_cwd =
            (!prepared.turn_environment.environment().is_remote()).then(|| prepared.cwd.clone());
        let timed_out_oneshot = completion
            .as_ref()
            .is_some_and(|completion| completion.timed_out);

        let (process_exited, exit_code) = if process_stored {
            let retention = if completion.is_some() {
                OutputRetention::UntilExit
            } else {
                OutputRetention::UntilCollected
            };
            match self
                .refresh_process_state(process_id, &process, retention)
                .await
            {
                ProcessStatus::Alive { exit_code, .. } => (false, exit_code),
                ProcessStatus::Exited { exit_code, entry } => {
                    entry.process.cancellation_token().cancel();
                    if let Err(message) =
                        finish_deferred_network_approval_after_process_exit_for_session(
                            Some(&context.session),
                            deferred_network_approval.take(),
                        )
                        .await
                    {
                        return Err(fail_process_with_message(entry.process.as_ref(), message));
                    }
                    if !timed_out_oneshot {
                        let (full_text, total_bytes, discarded_bytes) = {
                            let guard = output_buffer.lock().await;
                            (
                                String::from_utf8_lossy(guard.retained.as_bytes()).into_owned(),
                                guard.retained.total_bytes(),
                                guard.retained.discarded_bytes(),
                            )
                        };
                        let original_tokens = approx_tokens_from_u64(total_bytes);
                        let omitted = usize::try_from(discarded_bytes)
                            .ok()
                            .and_then(NonZeroUsize::new);
                        process
                            .check_for_sandbox_denial_with_text(&full_text)
                            .await
                            .map_err(|err| {
                                err.with_output_collection_metadata(original_tokens, omitted)
                            })?;
                    }
                    let metrics_sidecar = entry
                        .plugin_metrics_sidecar
                        .as_ref()
                        .and_then(take_plugin_metrics_sidecar);
                    finish_and_track_measurements(
                        metrics_sidecar,
                        exit_code.unwrap_or(-1),
                        &context.session,
                        &context.step_context.turn,
                        &model_context,
                        &context.call_id,
                    )
                    .await;
                    (true, exit_code)
                }
                ProcessStatus::Unknown => {
                    return Err(UnifiedExecError::UnknownProcessId { process_id });
                }
            }
        } else {
            // Short-lived command: emit the completed command item immediately
            // using the same helper as the background watcher.
            let (full_text, total_bytes, discarded_bytes) = {
                let guard = output_buffer.lock().await;
                (
                    String::from_utf8_lossy(guard.retained.as_bytes()).into_owned(),
                    guard.retained.total_bytes(),
                    guard.retained.discarded_bytes(),
                )
            };
            let finish_result = finish_deferred_network_approval_after_process_exit_for_session(
                Some(&context.session),
                deferred_network_approval.take(),
            )
            .await;
            if let Err(message) = finish_result {
                emit_failed_initial_exec_end_if_unstored(
                    process_stored,
                    process.sandbox_type(),
                    context,
                    &request,
                    cwd.clone(),
                    plugin_attribution.clone(),
                    Arc::clone(&output_buffer),
                    full_text,
                    message.clone(),
                    wall_time,
                )
                .await;
                self.release_process_id(request.process_id).await;
                return Err(fail_process_with_message(process.as_ref(), message));
            }
            let exit_code = process.exit_code();
            let exit = exit_code.unwrap_or(-1);
            initial_exec_command_guard
                .finish_plugin_metrics(context, exit)
                .await;
            emit_exec_end_for_unified_exec(
                process.sandbox_type(),
                Arc::clone(&context.session),
                Arc::clone(&context.step_context.turn),
                Arc::clone(&context.step_context.settings.model_info),
                context.call_id.clone(),
                command.clone(),
                cwd.clone(),
                Some(process_id.to_string()),
                plugin_attribution.clone(),
                Arc::clone(&output_buffer),
                full_text.clone(),
                exit,
                wall_time,
                process.timed_out(),
            )
            .await;

            if !timed_out_oneshot {
                let original_tokens = approx_tokens_from_u64(total_bytes);
                let omitted = usize::try_from(discarded_bytes)
                    .ok()
                    .and_then(NonZeroUsize::new);
                if let Err(err) = process.check_for_sandbox_denial_with_text(&full_text).await {
                    self.release_process_id(request.process_id).await;
                    return Err(err.with_output_collection_metadata(original_tokens, omitted));
                }
            }
            (true, exit_code)
        };

        if is_internal_unpaged {
            if !process_stored {
                self.release_process_id(process_id).await;
            }
            let (raw_output, original_token_count) = {
                let guard = output_buffer.lock().await;
                let total = guard.retained.total_bytes();
                (
                    CapturedStream::from(&guard.retained),
                    approx_tokens_from_u64(total),
                )
            };
            return Ok(ExecCommandToolOutput {
                event_call_id: context.call_id.clone(),
                chunk_id,
                wall_time,
                raw_output,
                truncation_policy,
                max_output_tokens: prepared.max_output_tokens,
                process_id: None,
                exit_code,
                original_token_count: Some(original_token_count),
                hook_command: Some(prepared.hook_command.clone()),
                tool_cwd,
                emit_post_tool_use: false,
            });
        }

        let stream_complete = process_exited
            && process
                .output_handles()
                .output_closed
                .load(Ordering::Acquire);
        let (original_token_count, page, retained_snapshot) = {
            let mut guard = output_buffer.lock().await;
            let unread_total = guard.unread_total_bytes();
            let original_token_count = approx_tokens_from_u64(unread_total);
            let page = guard.take_next_page(page_budget, stream_complete);
            let should_retain_snapshot = (!process_exited
                && (guard.retained.retained_bytes() > 0 || guard.retained.discarded_bytes() > 0))
                || (process_exited && page.has_more_output && !timed_out_oneshot);
            let snapshot = should_retain_snapshot.then(|| RetainedSnapshot {
                bytes: guard.retained.to_bytes(),
                retained_bytes: guard.retained.retained_bytes(),
                discarded_bytes: guard.retained.discarded_bytes(),
                read_offset: guard.read_offset,
                reported_discarded_bytes: guard.reported_discarded_bytes,
            });
            (original_token_count, page, snapshot)
        };

        let (response_process_id, emit_post_tool_use) = if !process_exited {
            if let Some(snapshot) = retained_snapshot {
                let meta = RetainedExecMeta {
                    process_id,
                    call_id: context.call_id.clone(),
                    environment_id: environment_id.clone(),
                    exit_code: None,
                    completed: false,
                    retained_bytes: snapshot.retained_bytes,
                    discarded_bytes: snapshot.discarded_bytes,
                    read_offset: snapshot.read_offset,
                    reported_discarded_bytes: snapshot.reported_discarded_bytes,
                    exhausted: false,
                    source_thread_id: None,
                    hook_command: Some(prepared.hook_command.clone()),
                    tool_cwd: tool_cwd.clone(),
                };
                self.persist_running_delta(
                    process_id,
                    &process,
                    /*prev*/ 0,
                    snapshot.bytes,
                    meta,
                )
                .await;
            }
            (Some(process_id), false)
        } else if page.has_more_output && !timed_out_oneshot {
            if let Some(snapshot) = retained_snapshot {
                let meta = RetainedExecMeta {
                    process_id,
                    call_id: context.call_id.clone(),
                    environment_id: environment_id.clone(),
                    exit_code,
                    completed: true,
                    retained_bytes: snapshot.retained_bytes,
                    discarded_bytes: snapshot.discarded_bytes,
                    read_offset: snapshot.read_offset,
                    reported_discarded_bytes: snapshot.reported_discarded_bytes,
                    exhausted: false,
                    source_thread_id: None,
                    hook_command: Some(prepared.hook_command.clone()),
                    tool_cwd: tool_cwd.clone(),
                };
                self.store_retained_exited(process_id, /*prev*/ 0, snapshot.bytes, meta)
                    .await;
            }
            (Some(process_id), true)
        } else {
            if !process_stored {
                self.release_process_id(process_id).await;
            }
            (None, true)
        };

        let response = ExecCommandToolOutput {
            event_call_id: context.call_id.clone(),
            chunk_id,
            wall_time,
            raw_output: CapturedStream::from(page.text),
            truncation_policy,
            max_output_tokens: prepared.max_output_tokens,
            process_id: response_process_id,
            exit_code,
            original_token_count: Some(original_token_count),
            hook_command: Some(prepared.hook_command.clone()),
            tool_cwd,
            emit_post_tool_use,
        };

        Ok(response)
    }

    #[tracing::instrument(
        name = "unified_exec.write_stdin",
        level = "info",
        skip_all,
        fields(
            conversation.id = %context.session.thread_id,
            turn_id = trace_id(&context.step_context.turn.sub_id),
            call_id = trace_id(&context.call_id),
            original_exec_call_id = tracing::field::Empty,
            unified_exec_process_id = request.terminal.process_id,
            interaction = if request.input.is_empty() { "poll" } else { "write" },
            outcome = tracing::field::Empty,
        )
    )]
    pub(crate) async fn write_stdin(
        &self,
        context: &UnifiedExecContext,
        request: WriteStdinRequest<'_>,
    ) -> Result<ExecCommandToolOutput, UnifiedExecError> {
        let result = self.write_stdin_inner(context, request).await;
        let outcome = match &result {
            Ok(output) if output.process_id.is_some() => "yielded",
            Ok(_) => "exited",
            Err(_) if context.cancellation_token.is_cancelled() => "cancelled",
            Err(_) => "failed",
        };
        tracing::Span::current().record("outcome", outcome);
        result
    }

    async fn write_stdin_inner(
        &self,
        context: &UnifiedExecContext,
        request: WriteStdinRequest<'_>,
    ) -> Result<ExecCommandToolOutput, UnifiedExecError> {
        let process_id = request.terminal.process_id;
        if let Some(call_id) = trace_id(&request.terminal.call_id) {
            tracing::Span::current().record("original_exec_call_id", call_id);
        }
        let locked_process = match &request.terminal.target {
            TerminalTarget::Retained { interaction_lock } => {
                return self
                    .write_stdin_retained(context, request, Arc::clone(interaction_lock))
                    .await;
            }
            TerminalTarget::Live(process) => process,
        };
        // Serialize writes and polls on the original terminal.
        let interaction_guard = locked_process.interaction_lock().lock_owned().await;
        // A queued write must observe strict review enabled while it was waiting.
        let strict_auto_review = context.step_context.turn.strict_auto_review_enabled();
        let approval = {
            let store = self.process_store.lock().await;
            match store
                .processes
                .get(&process_id)
                .filter(|entry| Arc::ptr_eq(&entry.process, locked_process))
            {
                Some(entry) => entry.stdin_approval(context, request.input, strict_auto_review)?,
                None => {
                    if let Some(retained) = store
                        .retained
                        .get(&process_id)
                        .filter(|entry| entry.meta.call_id == request.terminal.call_id)
                    {
                        let retained_lock = Arc::clone(&retained.interaction_lock);
                        drop(store);
                        drop(interaction_guard);
                        return self
                            .write_stdin_retained(context, request, retained_lock)
                            .await;
                    }
                    return Err(UnifiedExecError::UnknownProcessId { process_id });
                }
            }
        };
        let _interaction_guard = interaction_guard;
        if let Some((approval, approval_reason)) = approval {
            let unreviewable_input_error = || {
                UnifiedExecError::StdinApproval(ToolError::Rejected(
                    "terminal input and permission details are too large to review safely; use a smaller input or start a new terminal with fewer grants".to_string(),
                ))
            };
            let mut reviewed = codex_guardian_reviewer::guardian_approval_request_to_json(
                &approval
                    .clone()
                    .into_guardian_request(/*exec_command_cwd_convention*/ None)
                    .map_err(|err| {
                        UnifiedExecError::StdinApproval(ToolError::Rejected(err.to_string()))
                    })?,
            )
            .map_err(|_| unreviewable_input_error())?;
            // Launch context uses Guardian's normal request budget, not the stdin limit.
            if let Some(fields) = reviewed.as_object_mut() {
                fields.remove("original_command");
            }
            let reviewed =
                serde_json::to_string_pretty(&reviewed).map_err(|_| unreviewable_input_error())?;
            // Bound the input and permission details plus their reason, including
            // JSON escaping. Reject, never execute an unreviewed tail.
            let oversized =
                reviewed.len().saturating_add(approval_reason.len()) > MAX_STDIN_APPROVAL_BYTES;
            let size_check_result = if oversized {
                "over_limit"
            } else {
                "within_limit"
            };
            let input_kind = if request.input.chars().all(char::is_control) {
                "control"
            } else {
                "text"
            };
            context.step_context.session_telemetry.counter(
                "codex.unified_exec.stdin_review.size_check",
                /*inc*/ 1,
                &[("result", size_check_result), ("input_kind", input_kind)],
            );
            if oversized {
                return Err(unreviewable_input_error());
            }
            let approval_context = ApprovalContext {
                exec_requirement: None,
                review_context: GuardianReviewContext::from(&context.step_context),
                cancellation_token: Some(context.cancellation_token.clone()),
                call_id: context.call_id.clone(),
                tool_name: ToolName::plain("write_stdin"),
                strict_auto_review,
                approval_reason: Some(approval_reason),
                retry_reason: None,
                network_approval_context: None,
            };
            context
                .session
                .request_approval(approval, approval_context)
                .await
                .result
                .map_err(UnifiedExecError::StdinApproval)?;
        }

        // Revalidate the identity after approval: a removed process ID can be reused.
        let PreparedProcessHandles {
            process,
            output,
            pause_state,
            session,
            network_approval,
            call_id,
            environment_id,
            hook_command,
            tool_cwd,
            process_id,
            tty,
            mut persisted_bytes,
            mut persisted,
        } = self
            .prepare_process_handles(process_id, locked_process)
            .await?;
        context
            .session
            .services
            .rollout_thread_trace
            .record_stdin_target(
                &context.step_context.turn.sub_id,
                &context.call_id,
                &call_id,
            );
        if !request.input.is_empty() {
            if process.has_exited() {
                if output.drain_policy == OutputDrainPolicy::WaitForOutputClosure
                    && !output.output_closed.load(Ordering::Acquire)
                {
                    // Exit was observed remotely while output is still draining;
                    // skip writing to stdin so this call can finish collecting the tail.
                } else {
                    return Err(UnifiedExecError::ProcessExited { process_id });
                }
            } else if !tty {
                if request.input == INTERRUPT {
                    process.interrupt().await?;
                } else {
                    return Err(UnifiedExecError::StdinClosed);
                }
            } else {
                match process.write(request.input.as_bytes()).await {
                    Ok(()) => {
                        // Give the remote process a brief window to react so that we are
                        // more likely to capture its output in the poll below.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(err) => {
                        if process.has_exited() {
                            return Err(UnifiedExecError::ProcessExited { process_id });
                        }
                        if matches!(err, UnifiedExecError::ProcessFailed { .. }) {
                            process.terminate();
                            self.release_process(process_id, Some(locked_process)).await;
                        }
                        return Err(err);
                    }
                }
            }
        }

        let start = Instant::now();
        let page_budget =
            unified_exec_page_byte_budget(request.max_output_tokens, request.truncation_policy);
        let (has_full_buffered_page, has_partial_buffered_output) = if request.input.is_empty() {
            let stream_complete =
                process.has_exited() && output.output_closed.load(Ordering::Acquire);
            let guard = output.output_buffer.lock().await;
            let unread = guard.unread_total_bytes();
            let full_ready = stream_complete
                || (unread >= page_budget as u64 && guard.has_deliverable_unread(stream_complete));
            let partial_ready = !full_ready && guard.has_deliverable_unread(stream_complete);
            (full_ready, partial_ready)
        } else {
            (false, false)
        };
        if !has_full_buffered_page {
            let yield_time_ms = {
                // Empty polls use configurable background timeout bounds. Non-empty
                // writes (and polls with partial buffered output still awaiting closure)
                // keep a fixed max cap so interactive stdin and paging stay responsive.
                let time_ms = request.yield_time_ms.max(MIN_YIELD_TIME_MS);
                if request.input.is_empty() && !has_partial_buffered_output {
                    time_ms.clamp(MIN_EMPTY_YIELD_TIME_MS, self.max_write_stdin_yield_time_ms)
                } else {
                    time_ms.min(MAX_YIELD_TIME_MS)
                }
            };
            let deadline = start + Duration::from_millis(yield_time_ms);
            Self::wait_for_output_until_deadline(&output, pause_state, deadline).await;
        }
        let wall_time = Instant::now().saturating_duration_since(start);
        let chunk_id = generate_chunk_id();
        if network_approval
            .as_ref()
            .is_some_and(DeferredNetworkApproval::is_cancelled)
        {
            let message =
                network_denial_message_for_session(session.as_ref(), network_approval.clone())
                    .await;
            self.release_process(process_id, Some(locked_process)).await;
            return Err(fail_process_with_message(process.as_ref(), message));
        }
        if let Some(message) = process.failure_message() {
            let finish_result = finish_deferred_network_approval_for_session(
                session.as_ref(),
                network_approval.clone(),
            )
            .await;
            self.release_process(process_id, Some(locked_process)).await;
            if let Err(message) = finish_result {
                return Err(fail_process_with_message(process.as_ref(), message));
            }
            return Err(UnifiedExecError::process_failed(message));
        }

        // Only finalize remote sessions after both process exit and output collection.
        let status = self
            .refresh_process_state(process_id, locked_process, OutputRetention::UntilCollected)
            .await;
        let (process_exited, exit_code, event_call_id) = match status {
            ProcessStatus::Alive {
                exit_code, call_id, ..
            } => (false, exit_code, call_id),
            ProcessStatus::Exited { exit_code, entry } => {
                entry.process.cancellation_token().cancel();
                let call_id = entry.call_id.clone();
                persisted_bytes = entry.persisted_bytes;
                persisted = entry.persisted;
                if let Err(message) =
                    finish_network_approval_after_process_exit_for_entry(&entry).await
                {
                    return Err(fail_process_with_message(entry.process.as_ref(), message));
                }
                (true, exit_code, call_id)
            }
            ProcessStatus::Unknown => {
                if process.has_exited() {
                    (true, process.exit_code(), call_id)
                } else {
                    return Err(UnifiedExecError::UnknownProcessId { process_id });
                }
            }
        };

        let stream_complete = process_exited && output.output_closed.load(Ordering::Acquire);
        let (original_token_count, page, retained_snapshot) = {
            let mut guard = output.output_buffer.lock().await;
            let unread_total = guard.unread_total_bytes();
            let original_token_count = approx_tokens_from_u64(unread_total);
            let page = guard.take_next_page(page_budget, stream_complete);
            let should_retain_snapshot = (!process_exited
                && (guard.retained.retained_bytes() > 0 || guard.retained.discarded_bytes() > 0))
                || (process_exited && (page.has_more_output || persisted));
            let snapshot = should_retain_snapshot.then(|| RetainedSnapshot {
                bytes: guard.retained.to_bytes(),
                retained_bytes: guard.retained.retained_bytes(),
                discarded_bytes: guard.retained.discarded_bytes(),
                read_offset: guard.read_offset,
                reported_discarded_bytes: guard.reported_discarded_bytes,
            });
            (original_token_count, page, snapshot)
        };

        let (response_process_id, emit_post_tool_use) = if !process_exited {
            if let Some(snapshot) = retained_snapshot {
                let meta = RetainedExecMeta {
                    process_id,
                    call_id: event_call_id.clone(),
                    environment_id: environment_id.clone(),
                    exit_code: None,
                    completed: false,
                    retained_bytes: snapshot.retained_bytes,
                    discarded_bytes: snapshot.discarded_bytes,
                    read_offset: snapshot.read_offset,
                    reported_discarded_bytes: snapshot.reported_discarded_bytes,
                    exhausted: false,
                    source_thread_id: None,
                    hook_command: Some(hook_command.clone()),
                    tool_cwd: tool_cwd.clone(),
                };
                self.persist_running_delta(
                    process_id,
                    locked_process,
                    persisted_bytes,
                    snapshot.bytes,
                    meta,
                )
                .await;
            }
            (Some(process_id), false)
        } else if page.has_more_output {
            if let Some(snapshot) = retained_snapshot {
                let meta = RetainedExecMeta {
                    process_id,
                    call_id: event_call_id.clone(),
                    environment_id: environment_id.clone(),
                    exit_code,
                    completed: true,
                    retained_bytes: snapshot.retained_bytes,
                    discarded_bytes: snapshot.discarded_bytes,
                    read_offset: snapshot.read_offset,
                    reported_discarded_bytes: snapshot.reported_discarded_bytes,
                    exhausted: false,
                    source_thread_id: None,
                    hook_command: Some(hook_command.clone()),
                    tool_cwd: tool_cwd.clone(),
                };
                self.store_retained_exited(process_id, persisted_bytes, snapshot.bytes, meta)
                    .await;
            }
            (Some(process_id), true)
        } else {
            if persisted && let Some(snapshot) = retained_snapshot {
                let meta = RetainedExecMeta {
                    process_id,
                    call_id: event_call_id.clone(),
                    environment_id: environment_id.clone(),
                    exit_code,
                    completed: true,
                    retained_bytes: snapshot.retained_bytes,
                    discarded_bytes: snapshot.discarded_bytes,
                    read_offset: snapshot.read_offset,
                    reported_discarded_bytes: snapshot.reported_discarded_bytes,
                    exhausted: true,
                    source_thread_id: None,
                    hook_command: Some(hook_command.clone()),
                    tool_cwd: tool_cwd.clone(),
                };
                self.persist_exhausted(process_id, persisted_bytes, snapshot.bytes, meta)
                    .await;
            }
            (None, true)
        };

        let response = ExecCommandToolOutput {
            event_call_id,
            chunk_id,
            wall_time,
            raw_output: CapturedStream::from(page.text),
            truncation_policy: request.truncation_policy,
            max_output_tokens: request.max_output_tokens,
            process_id: response_process_id,
            exit_code,
            original_token_count: Some(original_token_count),
            hook_command: Some(hook_command),
            tool_cwd,
            emit_post_tool_use,
        };

        let should_emit_interaction = !request.input.is_empty()
            || (response.process_id.is_some() && response.exit_code.is_none());
        if should_emit_interaction
            && let Some(WriteStdinInteractionEvent { session, turn }) = request.interaction_event
        {
            let interaction = TerminalInteractionEvent {
                call_id: response.event_call_id.clone(),
                process_id: response
                    .process_id
                    .unwrap_or(request.terminal.process_id)
                    .to_string(),
                stdin: request.input.to_string(),
            };
            session
                .send_event(turn.as_ref(), EventMsg::TerminalInteraction(interaction))
                .await;
        }

        Ok(response)
    }

    async fn write_stdin_retained(
        &self,
        context: &UnifiedExecContext,
        request: WriteStdinRequest<'_>,
        interaction_lock: Arc<Mutex<()>>,
    ) -> Result<ExecCommandToolOutput, UnifiedExecError> {
        let process_id = request.terminal.process_id;
        let _interaction_guard = interaction_lock.lock_owned().await;
        if !request.input.is_empty() {
            return Err(UnifiedExecError::ProcessExited { process_id });
        }

        let (mut meta, bin_path, mem_bytes) = {
            let store = self.process_store.lock().await;
            let entry = store
                .retained
                .get(&process_id)
                .filter(|entry| !entry.meta.exhausted)
                .ok_or(UnifiedExecError::UnknownProcessId { process_id })?;
            (
                entry.meta.clone(),
                entry.bin_path.clone(),
                entry.bytes.clone(),
            )
        };

        context
            .session
            .services
            .rollout_thread_trace
            .record_stdin_target(
                &context.step_context.turn.sub_id,
                &context.call_id,
                &meta.call_id,
            );

        let start = Instant::now();
        let page_budget =
            unified_exec_page_byte_budget(request.max_output_tokens, request.truncation_policy);
        let total_unread = meta.retained_bytes.saturating_sub(meta.read_offset);
        let window_cap = page_budget.saturating_mul(2).saturating_add(16);

        let window = if let Some(bytes) = mem_bytes {
            let end = meta
                .read_offset
                .saturating_add(total_unread.min(window_cap))
                .min(bytes.len());
            let start_off = meta.read_offset.min(end);
            bytes[start_off..end].to_vec()
        } else {
            let read_offset = meta.read_offset;
            let retained_bytes = meta.retained_bytes;
            tokio::task::spawn_blocking(move || {
                read_retained_window(&bin_path, read_offset, retained_bytes, window_cap)
            })
            .await
            .map_err(|err| UnifiedExecError::process_failed(err.to_string()))?
            .map_err(|err| {
                UnifiedExecError::process_failed(format!(
                    "failed to read retained output for process {process_id}: {err}"
                ))
            })?
        };

        let effective_unread = if window.len() < total_unread.min(window_cap) {
            // File was shorter than recorded metadata; clamp so paging terminates cleanly.
            meta.retained_bytes = meta.read_offset.saturating_add(window.len());
            window.len()
        } else {
            total_unread
        };
        let unread_total_bytes = (effective_unread as u64).saturating_add(
            meta.discarded_bytes
                .saturating_sub(meta.reported_discarded_bytes),
        );
        let original_token_count = approx_tokens_from_u64(unread_total_bytes);
        let page = slice_next_page(
            &window,
            effective_unread,
            meta.discarded_bytes,
            meta.reported_discarded_bytes,
            page_budget,
            /*stream_complete*/ true,
        );
        let wall_time = start.elapsed();

        meta.read_offset = meta.read_offset.saturating_add(page.consumed_bytes);
        meta.reported_discarded_bytes = page.reported_discarded_bytes;
        meta.exhausted = !page.has_more_output;

        if let Some(storage) = self.storage.clone()
            && !storage.is_ephemeral()
        {
            let meta_for_disk = meta.clone();
            match tokio::task::spawn_blocking(move || storage.persist_meta(&meta_for_disk)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    warn!(process_id, "failed to update retained exec metadata: {err}");
                }
                Err(err) => {
                    warn!(
                        process_id,
                        "retained exec metadata persistence task failed: {err}"
                    );
                }
            }
        }

        {
            let mut store = self.process_store.lock().await;
            if meta.exhausted {
                store.retained.remove(&process_id);
            } else if let Some(entry) = store.retained.get_mut(&process_id) {
                entry.meta = meta.clone();
            }
        }

        Ok(ExecCommandToolOutput {
            event_call_id: meta.call_id,
            chunk_id: generate_chunk_id(),
            wall_time,
            raw_output: CapturedStream::from(page.text),
            truncation_policy: request.truncation_policy,
            max_output_tokens: request.max_output_tokens,
            process_id: page.has_more_output.then_some(process_id),
            exit_code: meta.exit_code,
            original_token_count: Some(original_token_count),
            hook_command: meta.hook_command,
            tool_cwd: meta.tool_cwd,
            emit_post_tool_use: false,
        })
    }

    async fn persist_running_delta(
        &self,
        process_id: i32,
        expected_process: &Arc<UnifiedExecProcess>,
        previously_persisted_bytes: usize,
        bytes: Vec<u8>,
        meta: RetainedExecMeta,
    ) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        if storage.is_ephemeral() {
            return;
        }
        let persisted_len = match tokio::task::spawn_blocking(move || {
            storage.persist_delta(process_id, previously_persisted_bytes, &bytes, &meta)
        })
        .await
        {
            Ok(Ok(written)) => written,
            Ok(Err(err)) => {
                warn!(process_id, "failed to persist running exec output: {err}");
                return;
            }
            Err(err) => {
                warn!(
                    process_id,
                    "running exec output persistence task failed: {err}"
                );
                return;
            }
        };
        let mut store = self.process_store.lock().await;
        if let Some(entry) = store
            .processes
            .get_mut(&process_id)
            .filter(|entry| Arc::ptr_eq(&entry.process, expected_process))
        {
            entry.persisted_bytes = persisted_len;
            entry.persisted = true;
        }
    }

    async fn store_retained_exited(
        &self,
        process_id: i32,
        previously_persisted_bytes: usize,
        bytes: Vec<u8>,
        meta: RetainedExecMeta,
    ) {
        let (bin_path, mem_bytes) = if let Some(storage) = self.storage.clone()
            && !storage.is_ephemeral()
        {
            let bin_path = storage.bin_path_for(process_id, /*source_thread_id*/ None);
            let meta_for_disk = meta.clone();
            let bytes_for_disk = bytes;
            let persisted = tokio::task::spawn_blocking(move || {
                match storage.persist_delta(
                    process_id,
                    previously_persisted_bytes,
                    &bytes_for_disk,
                    &meta_for_disk,
                ) {
                    Ok(_) => Ok(()),
                    Err(err) => Err((err, bytes_for_disk)),
                }
            })
            .await;
            match persisted {
                Ok(Ok(())) => (bin_path, None),
                Ok(Err((err, fallback_bytes))) => {
                    warn!(process_id, "failed to persist exited exec output: {err}");
                    (bin_path, Some(Arc::new(fallback_bytes)))
                }
                Err(err) => {
                    warn!(
                        process_id,
                        "exited exec output persistence task failed: {err}"
                    );
                    (bin_path, None)
                }
            }
        } else {
            (PathBuf::new(), Some(Arc::new(bytes)))
        };

        let mut store = self.process_store.lock().await;
        store.reserved_process_ids.insert(process_id);
        store.retained.insert(
            process_id,
            RetainedEntry {
                meta,
                bin_path,
                bytes: mem_bytes,
                interaction_lock: Arc::new(Mutex::new(())),
            },
        );
    }

    async fn persist_exhausted(
        &self,
        process_id: i32,
        previously_persisted_bytes: usize,
        bytes: Vec<u8>,
        meta: RetainedExecMeta,
    ) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        if storage.is_ephemeral() {
            return;
        }
        match tokio::task::spawn_blocking(move || {
            storage.persist_delta(process_id, previously_persisted_bytes, &bytes, &meta)
        })
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                warn!(
                    process_id,
                    "failed to persist exhausted exec metadata: {err}"
                );
            }
            Err(err) => {
                warn!(
                    process_id,
                    "exhausted exec metadata persistence task failed: {err}"
                );
            }
        }
    }

    async fn refresh_process_state(
        &self,
        process_id: i32,
        expected_process: &Arc<UnifiedExecProcess>,
        retention: OutputRetention,
    ) -> ProcessStatus {
        let mut store = self.process_store.lock().await;
        let Some(entry) = store
            .processes
            .get_mut(&process_id)
            .filter(|entry| Arc::ptr_eq(&entry.process, expected_process))
        else {
            return ProcessStatus::Unknown;
        };

        let exit_code = entry.process.exit_code();
        let process_id = entry.process_id;

        if entry.process.has_exited() {
            let output = entry.process.output_handles();
            // Never wait for output under the shared process-store lock. Retain open remote
            // output streams for another poll; once output is closed, any unread bytes move
            // to retained pagination on exit.
            if retention == OutputRetention::UntilCollected
                && output.drain_policy == OutputDrainPolicy::WaitForOutputClosure
                && !entry.process.termination_requested.load(Ordering::Acquire)
                && !output.output_closed.load(Ordering::Acquire)
            {
                return ProcessStatus::Alive {
                    exit_code: None,
                    call_id: entry.call_id.clone(),
                };
            }
            let Some(entry) = store.remove(process_id) else {
                return ProcessStatus::Unknown;
            };
            ProcessStatus::Exited {
                exit_code,
                entry: Box::new(entry),
            }
        } else {
            ProcessStatus::Alive {
                exit_code,
                call_id: entry.call_id.clone(),
            }
        }
    }

    async fn prepare_process_handles(
        &self,
        process_id: i32,
        expected_process: &Arc<UnifiedExecProcess>,
    ) -> Result<PreparedProcessHandles, UnifiedExecError> {
        let mut store = self.process_store.lock().await;
        let entry = store
            .processes
            .get_mut(&process_id)
            .ok_or(UnifiedExecError::UnknownProcessId { process_id })?;
        if !Arc::ptr_eq(&entry.process, expected_process) {
            return Err(UnifiedExecError::UnknownProcessId { process_id });
        }
        entry.last_used = Instant::now();
        let output = entry.process.output_handles().clone();
        let pause_state = entry
            .session
            .upgrade()
            .map(|session| session.subscribe_elicitation_pause_state());
        let session = entry.session.upgrade();

        Ok(PreparedProcessHandles {
            process: Arc::clone(&entry.process),
            output,
            pause_state,
            session,
            network_approval: entry.network_approval.clone(),
            call_id: entry.call_id.clone(),
            environment_id: entry.environment_id.clone(),
            hook_command: entry.hook_command.clone(),
            // Capture the original process's locality before it can be removed
            // from the store. A later turn may select a different environment.
            tool_cwd: (entry.environment_id == codex_exec_server::LOCAL_ENVIRONMENT_ID)
                .then(|| entry.cwd.clone()),
            process_id: entry.process_id,
            tty: entry.tty,
            persisted_bytes: entry.persisted_bytes,
            persisted: entry.persisted,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn store_process(
        &self,
        process: Arc<UnifiedExecProcess>,
        context: &UnifiedExecContext,
        command: &[String],
        hook_command: String,
        cwd: PathUri,
        environment_id: String,
        permissions: super::TerminalPermissions,
        plugin_attribution: Option<PluginCommandAttribution>,
        started_at: Instant,
        process_id: i32,
        tty: bool,
        network_approval: Option<DeferredNetworkApproval>,
        network_denial_monitor: Option<tokio::task::JoinHandle<()>>,
        metrics_sidecar: Option<PluginMetricsSidecar>,
        output_buffer: Arc<tokio::sync::Mutex<OutputBuffers>>,
        initial_exec_command_active: Arc<AtomicBool>,
    ) {
        let plugin_metrics_sidecar =
            metrics_sidecar.map(|sidecar| Arc::new(std::sync::Mutex::new(Some(sidecar))));
        let entry = ProcessEntry {
            process: Arc::clone(&process),
            plugin_metrics_sidecar: plugin_metrics_sidecar.clone(),
            call_id: context.call_id.clone(),
            process_id,
            cwd: cwd.clone(),
            initial_exec_command_active,
            hook_command,
            tty,
            environment_id,
            permissions,
            network_approval,
            session: Arc::downgrade(&context.session),
            last_used: started_at,
            persisted_bytes: 0,
            persisted: false,
        };
        let pruned_entry = {
            let mut store = self.process_store.lock().await;
            let pruned_entry = Self::prune_processes_if_needed(&mut store);
            store.processes.insert(process_id, entry);
            pruned_entry
        };
        spawn_exit_watcher(
            Arc::clone(&process),
            context,
            command.to_vec(),
            cwd,
            process_id,
            plugin_attribution,
            output_buffer,
            started_at,
            network_denial_monitor,
            plugin_metrics_sidecar,
        );
        // Install the watcher before cancellable cleanup, and outside the process-store lock.
        if let Some(pruned_entry) = pruned_entry {
            unregister_network_approval_for_entry(&pruned_entry).await;
            pruned_entry.process.terminate();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn open_session_with_exec_env(
        &self,
        process_id: i32,
        tool_ctx: &ToolCtx,
        request: ExecRequest,
        network: Option<&NetworkProxy>,
        exec_server_env_config: Option<ExecServerEnvConfig>,
        shell_snapshot: Option<codex_exec_server::ShellSnapshotRequest>,
        windows_sandbox_proxy_settings_mode: codex_sandboxing::WindowsSandboxProxySettingsMode,
        tty: bool,
        environment: &codex_exec_server::Environment,
    ) -> Result<UnifiedExecProcess, ToolError> {
        let network_policy_decider = request
            .exec_server_network_proxy
            .as_ref()
            .filter(|launch| launch.policy_decision_timeout_ms.is_some())
            .and_then(|launch| network?.remote_policy_decider(launch.proxy.allow_local_binding));
        let request = ExecRequest {
            exec_server_env_config,
            exec_server_shell_snapshot: shell_snapshot,
            ..request
        };
        self.open_session_with_prepared_exec_env(
            process_id,
            &request,
            Some(tool_ctx),
            windows_sandbox_proxy_settings_mode,
            network_policy_decider,
            tty,
            environment,
        )
        .await
        .map_err(|err| match err {
            UnifiedExecError::SandboxDenied { output, .. } => {
                ToolError::Codex(CodexErr::Sandbox(SandboxErr::Denied {
                    output: Box::new(output),
                    network_policy_decision: None,
                }))
            }
            other => ToolError::Rejected(other.to_string()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn open_session_with_prepared_exec_env(
        &self,
        process_id: i32,
        request: &ExecRequest,
        tool_ctx: Option<&ToolCtx>,
        windows_sandbox_proxy_settings_mode: codex_sandboxing::WindowsSandboxProxySettingsMode,
        network_policy_decider: Option<Arc<dyn NetworkPolicyDecider>>,
        tty: bool,
        environment: &codex_exec_server::Environment,
    ) -> Result<UnifiedExecProcess, UnifiedExecError> {
        if environment.is_remote() || request.exec_server_shell_snapshot.is_some() {
            let backend = environment.get_exec_backend();
            let params = exec_server_params_for_request(
                process_id,
                request,
                tool_ctx,
                windows_sandbox_proxy_settings_mode,
                tty,
            );
            // Sandbox retries can reuse the public ID for a new executor process.
            tracing::event!(
                name: "codex.unified_exec.process_start_requested",
                target: "codex_otel.trace_safe",
                tracing::Level::INFO,
                event.name = "codex.unified_exec.process_start_requested",
                unified_exec_process_id = process_id,
                process.id = params.process_id.as_str(),
            );
            let started = match network_policy_decider {
                Some(decider) => {
                    backend
                        .start_with_network_policy_decider(params, decider)
                        .await
                }
                None => backend.start(params).await,
            }
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
            let output_drain_policy = if environment.is_remote() {
                OutputDrainPolicy::WaitForOutputClosure
            } else {
                OutputDrainPolicy::BoundedAfterExit
            };
            return UnifiedExecProcess::from_exec_server_started(started, output_drain_policy)
                .await;
        }

        // TODO(anp): Keep PathUri through the local PTY/process launch boundary.
        let native_cwd = request
            .cwd
            .to_abs_path()
            .map_err(|_| UnifiedExecError::ForeignPath {
                path: request.cwd.clone(),
            })?;

        let (program, args) = request
            .program_and_args()
            .ok_or(UnifiedExecError::MissingCommandLine)?;
        let network_proxy_restricting_sid = {
            #[cfg(target_os = "windows")]
            {
                if request.sandbox == codex_sandboxing::SandboxType::WindowsRestrictedToken {
                    request
                        .network
                        .as_ref()
                        .map(|network| {
                            network
                                .network_proxy_restricting_sid(
                                    request.network_environment_id.as_deref(),
                                )
                                .ok_or_else(|| {
                                    UnifiedExecError::create_process(
                                        "managed Windows proxy route is missing its restricting SID"
                                            .to_string(),
                                    )
                                })
                        })
                        .transpose()?
                } else {
                    None
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                None::<String>
            }
        };
        let windows_sandbox =
            if request.sandbox == codex_sandboxing::SandboxType::WindowsRestrictedToken {
                Some(codex_sandboxing::WindowsSandboxSpawnRequest {
                    permission_profile: &request.permission_profile,
                    workspace_roots: &request.windows_sandbox_workspace_roots,
                    windows_sandbox_level: request.windows_sandbox_level,
                    proxy_enforced: request.network.is_some(),
                    network_proxy_restricting_sid: network_proxy_restricting_sid.as_deref(),
                    proxy_settings_mode: windows_sandbox_proxy_settings_mode,
                    filesystem_overrides: request.windows_sandbox_filesystem_overrides.as_ref(),
                })
            } else {
                None
            };
        let spawn_result = codex_sandboxing::spawn_process(codex_sandboxing::SpawnRequest {
            program,
            args,
            cwd: native_cwd.as_path(),
            env: &request.env,
            arg0: &request.arg0,
            sandbox: request.sandbox,
            windows_sandbox,
            tty,
            stdin_open: tty,
            inherited_fds: codex_utils_pty::ChildFds::Inherited(&[]),
        })
        .await;
        let spawned =
            spawn_result.map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        UnifiedExecProcess::from_spawned(spawned, request.sandbox).await
    }

    #[tracing::instrument(
        name = "unified_exec.open_session",
        level = "info",
        skip_all,
        fields(outcome = tracing::field::Empty)
    )]
    pub(super) async fn open_session_with_sandbox(
        &self,
        request: &ExecCommandRequest,
        context: &UnifiedExecContext,
    ) -> Result<(UnifiedExecAttempt, Option<DeferredNetworkApproval>), UnifiedExecError> {
        let prepared = &request.prepared;
        let shell_environment_policy = prepared.turn_environment.shell_environment_policy();
        let local_policy_env = create_env(shell_environment_policy, /*thread_id*/ None);
        let mut env = local_policy_env.clone();
        #[cfg(windows)]
        env.retain(|name, _| !name.eq_ignore_ascii_case(CODEX_THREAD_ID_ENV_VAR));
        env.insert(
            CODEX_THREAD_ID_ENV_VAR.to_string(),
            context.session.thread_id.to_string(),
        );
        set_tool_call_id_env_var(&mut env, Some(&context.call_id));
        inject_session_env(&mut env, context.session.session_id());
        inject_apply_patch_env(&mut env);
        let active_permission_profile = prepared.turn_environment.active_permission_profile();
        inject_permission_profile_env(&mut env, active_permission_profile.as_ref());
        let mut env = apply_unified_exec_env(env);
        strip_output_env(&mut env);
        let mut explicit_env_overrides = shell_environment_policy.r#set.clone();
        explicit_env_overrides.insert(
            SESSION_TMP_ENV_VAR.to_string(),
            crate::exec_env::session_tmp_path(context.session.session_id())
                .to_string_lossy()
                .into_owned(),
        );
        strip_output_env(&mut explicit_env_overrides);
        let exec_server_env_config = ExecServerEnvConfig {
            policy: exec_env_policy_from_shell_policy(shell_environment_policy),
            local_policy_env,
        };
        let shell_snapshot = shell_snapshot_request(prepared, context);
        let mut orchestrator = ToolOrchestrator::new();
        let mut runtime = UnifiedExecRuntime::new(self);
        let exec_approval_requirement = prepared
            .approval_requirement(&context.session, &context.step_context)
            .await;
        let req = UnifiedExecToolRequest {
            prepared,
            process_id: request.process_id,
            env,
            exec_server_env_config: Some(exec_server_env_config),
            shell_snapshot,
            explicit_env_overrides,
            network: request.network.clone(),
            exec_approval_requirement,
        };
        let tool_ctx = ToolCtx {
            session: context.session.clone(),
            step_context: Arc::clone(&context.step_context),
            cancellation_token: context.cancellation_token.clone(),
            call_id: context.call_id.clone(),
            tool_name: ToolName::plain("exec_command"),
        };
        let result = orchestrator
            .run(&mut runtime, &req, &tool_ctx)
            .await
            .map(|result| (result.output, result.deferred_network_approval))
            .map_err(|err| match err {
                ToolError::Codex(err) => match err.details() {
                    CodexErrorDetails::Sandbox(SandboxErr::Denied { output, .. }) => {
                        let output = output.as_ref().clone();
                        let message = if output.aggregated_output.text.is_empty() {
                            let exit_code = output.exit_code;
                            format!("Process exited with code {exit_code}")
                        } else {
                            output.aggregated_output.text.clone()
                        };
                        UnifiedExecError::sandbox_denied(message, output)
                    }
                    _ => UnifiedExecError::create_process(format!("{err:?}")),
                },
                other => UnifiedExecError::create_process(format!("{other:?}")),
            });
        let outcome = match &result {
            Ok(_) => "completed",
            Err(_) if context.cancellation_token.is_cancelled() => "cancelled",
            Err(_) => "failed",
        };
        tracing::Span::current().record("outcome", outcome);
        result
    }

    #[tracing::instrument(
        name = "unified_exec.collect_output",
        level = "info",
        skip_all,
        fields(
            outcome = tracing::field::Empty,
            stop_reason = tracing::field::Empty,
            exit_signaled = tracing::field::Empty,
            output_closed = tracing::field::Empty,
        )
    )]
    pub(super) async fn wait_for_output_until_deadline<const MAX_BYTES: usize>(
        output: &OutputHandles<MAX_BYTES>,
        mut pause_state: Option<watch::Receiver<bool>>,
        mut deadline: Instant,
    ) {
        const POST_EXIT_CLOSE_WAIT_CAP: Duration = Duration::from_millis(50);

        enum StopReason {
            OutputClosed,
            Deadline,
            PostExitDeadline,
        }

        let OutputHandles {
            output_notify,
            output_closed,
            output_closed_notify,
            cancellation_token,
            ..
        } = output;
        let mut exit_signal_received = cancellation_token.is_cancelled();
        let mut post_exit_deadline: Option<Instant> = None;
        let stop_reason = loop {
            Self::extend_deadlines_while_paused(
                &mut pause_state,
                &mut deadline,
                &mut post_exit_deadline,
            )
            .await;

            exit_signal_received |= cancellation_token.is_cancelled();
            if exit_signal_received && output_closed.load(Ordering::Acquire) {
                break StopReason::OutputClosed;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining == Duration::ZERO {
                break StopReason::Deadline;
            }

            if exit_signal_received {
                let now = Instant::now();
                let close_wait_deadline = *post_exit_deadline
                    .get_or_insert_with(|| now + remaining.min(POST_EXIT_CLOSE_WAIT_CAP));
                let close_wait_remaining = close_wait_deadline.saturating_duration_since(now);
                if close_wait_remaining == Duration::ZERO {
                    break StopReason::PostExitDeadline;
                }
                let notified = output_notify.notified();
                let closed = output_closed_notify.notified();
                tokio::pin!(notified);
                tokio::pin!(closed);
                notified.as_mut().enable();
                closed.as_mut().enable();
                if output_closed.load(Ordering::Acquire) {
                    break StopReason::OutputClosed;
                }
                tokio::select! {
                    _ = &mut notified => {}
                    _ = &mut closed => {}
                    _ = tokio::time::sleep(close_wait_remaining) => break StopReason::PostExitDeadline,
                    _ = Self::wait_for_pause_change(pause_state.as_ref()) => {}
                }
                continue;
            }

            let exit_notified = cancellation_token.cancelled();
            tokio::pin!(exit_notified);
            tokio::select! {
                _ = &mut exit_notified => exit_signal_received = true,
                _ = tokio::time::sleep(remaining) => break StopReason::Deadline,
                _ = Self::wait_for_pause_change(pause_state.as_ref()) => {}
            }
        };

        let span = tracing::Span::current();
        span.record(
            "stop_reason",
            match stop_reason {
                StopReason::OutputClosed => "output_closed",
                StopReason::Deadline => "deadline",
                StopReason::PostExitDeadline => "post_exit_deadline",
            },
        );
        span.record(
            "exit_signaled",
            exit_signal_received || cancellation_token.is_cancelled(),
        );
        span.record("output_closed", output_closed.load(Ordering::Acquire));
        span.record("outcome", "completed");
    }

    #[cfg(test)]
    pub(super) async fn collect_output_until_deadline<const MAX_BYTES: usize>(
        output: &OutputHandles<MAX_BYTES>,
        pause_state: Option<watch::Receiver<bool>>,
        deadline: Instant,
    ) -> super::output::HeadRetainedBuffer<MAX_BYTES> {
        Self::wait_for_output_until_deadline(output, pause_state, deadline).await;
        output.output_buffer.lock().await.retained.clone()
    }

    async fn extend_deadlines_while_paused(
        pause_state: &mut Option<watch::Receiver<bool>>,
        deadline: &mut Instant,
        post_exit_deadline: &mut Option<Instant>,
    ) {
        let Some(receiver) = pause_state.as_mut() else {
            return;
        };
        if !*receiver.borrow() {
            return;
        }

        let paused_at = Instant::now();
        while *receiver.borrow() {
            if receiver.changed().await.is_err() {
                break;
            }
        }

        let paused_for = paused_at.elapsed();
        *deadline += paused_for;
        if let Some(post_exit_deadline) = post_exit_deadline.as_mut() {
            *post_exit_deadline += paused_for;
        }
    }

    async fn wait_for_pause_change(pause_state: Option<&watch::Receiver<bool>>) {
        match pause_state {
            Some(pause_state) => {
                let mut receiver = pause_state.clone();
                let _ = receiver.changed().await;
            }
            None => std::future::pending::<()>().await,
        }
    }

    fn prune_processes_if_needed(store: &mut ProcessStore) -> Option<ProcessEntry> {
        if store.processes.len() < MAX_UNIFIED_EXEC_PROCESSES {
            return None;
        }

        let mut meta: Vec<(i32, Instant, bool)> = store
            .processes
            .iter()
            .map(|(id, entry)| (*id, entry.last_used, entry.process.has_exited()))
            .collect();
        let mut found_locked_exited_process = false;

        while let Some(process_id) = Self::process_id_to_prune_from_meta(&meta) {
            let candidate_process = store
                .processes
                .get(&process_id)
                .map(|entry| Arc::clone(&entry.process));
            let candidate_has_exited = candidate_process
                .as_ref()
                .is_some_and(|process| process.has_exited());
            if found_locked_exited_process && !candidate_has_exited {
                // The store may temporarily exceed its soft cap while an exited
                // process is publishing its terminal event. Do not evict a live
                // process just because that exited process is briefly locked.
                return None;
            }

            // Do not prune processes while write_stdin or terminal event
            // publication holds their interaction lock.
            if let Some(interaction_lock) = candidate_process
                .as_ref()
                .map(|process| process.interaction_lock())
                && let Ok(_interaction_guard) = interaction_lock.try_lock_owned()
            {
                return store.remove(process_id);
            }
            found_locked_exited_process |= candidate_has_exited
                || candidate_process.is_some_and(|process| process.has_exited());
            meta.retain(|(id, _, _)| *id != process_id);
        }

        None
    }

    // Centralized pruning policy so we can easily swap strategies later.
    fn process_id_to_prune_from_meta(meta: &[(i32, Instant, bool)]) -> Option<i32> {
        if meta.is_empty() {
            return None;
        }

        let mut by_recency = meta.to_vec();
        by_recency.sort_by_key(|(_, last_used, _)| Reverse(*last_used));
        let protected: HashSet<i32> = by_recency
            .iter()
            .take(8)
            .map(|(process_id, _, _)| *process_id)
            .collect();

        let mut lru = meta.to_vec();
        lru.sort_by_key(|(_, last_used, _)| *last_used);

        if let Some((process_id, _, _)) = lru
            .iter()
            .find(|(process_id, _, exited)| !protected.contains(process_id) && *exited)
        {
            return Some(*process_id);
        }

        lru.into_iter()
            .find(|(process_id, _, _)| !protected.contains(process_id))
            .map(|(process_id, _, _)| process_id)
    }

    pub(crate) async fn terminate_all_processes(&self) {
        let entries: Vec<ProcessEntry> = {
            let mut processes = self.process_store.lock().await;
            let entries: Vec<ProcessEntry> = processes
                .processes
                .drain()
                .map(|(_, entry)| entry)
                .collect();
            processes.retained.clear();
            processes.reserved_process_ids.clear();
            entries
        };

        for entry in entries {
            unregister_network_approval_for_entry(&entry).await;
            entry.process.terminate();
        }
    }

    pub(crate) async fn resolve_terminal(
        &self,
        process_id: i32,
    ) -> Result<TerminalSession, UnifiedExecError> {
        let store = self.process_store.lock().await;
        if let Some(entry) = store.processes.get(&process_id) {
            return Ok(TerminalSession {
                process_id,
                target: TerminalTarget::Live(Arc::clone(&entry.process)),
                call_id: entry.call_id.clone(),
                environment_id: entry.environment_id.clone(),
            });
        }
        if let Some(entry) = store
            .retained
            .get(&process_id)
            .filter(|entry| !entry.meta.exhausted)
        {
            return Ok(TerminalSession {
                process_id,
                target: TerminalTarget::Retained {
                    interaction_lock: Arc::clone(&entry.interaction_lock),
                },
                call_id: entry.meta.call_id.clone(),
                environment_id: entry.meta.environment_id.clone(),
            });
        }
        Err(UnifiedExecError::UnknownProcessId { process_id })
    }

    pub(crate) async fn list_processes(&self) -> Vec<BackgroundTerminalInfo> {
        let store = self.process_store.lock().await;
        let mut entries = store
            .processes
            .values()
            .filter(|entry| !entry.process.has_exited())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.process_id);
        entries
            .into_iter()
            .map(|entry| BackgroundTerminalInfo {
                item_id: entry.call_id.clone(),
                process_id: entry.process_id.to_string(),
                command: entry.hook_command.clone(),
                cwd: entry.cwd.clone(),
            })
            .collect()
    }

    pub(crate) async fn terminate_process(&self, process_id: i32) -> bool {
        let (process, already_exited) = {
            let store = self.process_store.lock().await;
            let Some(entry) = store.processes.get(&process_id) else {
                return false;
            };
            (Arc::clone(&entry.process), entry.process.has_exited())
        };

        if already_exited {
            // Stop the output producer even when only its inherited pipes remain open.
            process.terminate();
        } else if process.terminate_confirmed().await.is_err() {
            return false;
        }

        let entry = {
            let mut store = self.process_store.lock().await;
            let Some(entry) = store.processes.get(&process_id) else {
                return true;
            };
            if !Arc::ptr_eq(&entry.process, &process) {
                return true;
            }
            if entry.initial_exec_command_active.load(Ordering::Acquire) {
                return true;
            }
            let Some(entry) = store.remove(process_id) else {
                return false;
            };
            entry
        };

        unregister_network_approval_for_entry(&entry).await;
        true
    }
}

#[derive(PartialEq, Eq)]
enum OutputRetention {
    UntilExit,
    UntilCollected,
}

enum ProcessStatus {
    Alive {
        exit_code: Option<i32>,
        call_id: String,
    },
    Exited {
        exit_code: Option<i32>,
        entry: Box<ProcessEntry>,
    },
    Unknown,
}

#[cfg(test)]
#[path = "process_manager_tests.rs"]
mod tests;
