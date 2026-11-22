//! Unified Exec: interactive process execution orchestrated with approvals + sandboxing.
//!
//! Responsibilities
//! - Manages interactive processes (create, reuse, buffer output with caps).
//! - Supports completion-only calls that terminate on timeout or cancellation.
//! - Uses the shared ToolOrchestrator to handle approval, sandbox selection, and
//!   retry semantics in a single, descriptive flow.
//! - Spawns the PTY from a sandbox-transformed `ExecRequest`; on sandbox denial,
//!   retries without sandbox when policy allows (no re‑prompt thanks to caching).
//! - Uses the shared `is_likely_sandbox_denied` heuristic to keep denial messages
//!   consistent with other exec paths.
//!
//! Flow at a glance (open process)
//! 1) Build a small request `{ command, cwd }`.
//! 2) Orchestrator: approval (bypass/cache/prompt) → select sandbox → run.
//! 3) Prepare the approved sandbox on the execution host, then spawn the PTY.
//! 4) On sandbox denial, the orchestrator may authorize a retry with different permissions.
//! 5) Process handle is returned with streaming output + metadata.
//!
//! This keeps policy logic and user interaction centralized while the PTY/process
//! concerns remain isolated here. The implementation is split between:
//! - `process.rs`: PTY process lifecycle + output buffering.
//! - `process_state.rs`: shared exit/failure state for local and remote processes.
//! - `process_manager.rs`: orchestration (approvals, sandboxing, reuse) and request handling.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;

use codex_network_proxy::NetworkProxy;
use codex_protocol::ThreadId;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_path_uri::PathUri;
use rand::Rng;
use rand::rng;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use crate::tools::handlers::unified_exec::preparation::PreparedExecCommand;
use crate::tools::network_approval::DeferredNetworkApproval;
use codex_core_plugins::PluginMetricsSidecar;

mod async_watcher;
mod command_sessions;
mod errors;
mod oneshot;
pub(crate) mod output;
mod process;
mod process_manager;
pub(crate) use process_manager::exec_env_policy_from_shell_policy;
mod process_state;
pub(crate) mod retained;
mod shell_snapshot;
mod stdin_approval;

pub(crate) fn set_deterministic_process_ids_for_tests(enabled: bool) {
    process_manager::set_deterministic_process_ids_for_tests(enabled);
}

pub(crate) use command_sessions::command_sessions_state;
pub(crate) use errors::UnifiedExecError;
pub(crate) use output::MAX_RETAINED_OUTPUT_BYTES;
pub(crate) use process::UnifiedExecProcess;
pub(crate) use stdin_approval::TerminalPermissions;
pub(crate) use stdin_approval::TerminalSandboxSource;

pub(crate) const MIN_YIELD_TIME_MS: u64 = 250;
pub(crate) const WINDOWS_INITIAL_EXEC_YIELD_TIME_FLOOR_MS: u64 = 10_000;
// Minimum yield time for an empty `write_stdin`.
pub(crate) const MIN_EMPTY_YIELD_TIME_MS: u64 = 5_000;
pub(crate) const MAX_YIELD_TIME_MS: u64 = 30_000;
pub(crate) const DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: usize = 10_000;
pub(crate) const MAX_UNIFIED_EXEC_PROCESSES: usize = 64;

const MAX_TRACE_ID_BYTES: usize = 256;

fn trace_id(id: &str) -> Option<&str> {
    (!id.is_empty() && id.len() <= MAX_TRACE_ID_BYTES).then_some(id)
}

pub(crate) struct UnifiedExecContext {
    pub session: Arc<Session>,
    pub step_context: Arc<StepContext>,
    pub cancellation_token: CancellationToken,
    pub call_id: String,
}

impl UnifiedExecContext {
    pub fn new(
        session: Arc<Session>,
        step_context: Arc<StepContext>,
        cancellation_token: CancellationToken,
        call_id: String,
    ) -> Self {
        Self {
            session,
            step_context,
            cancellation_token,
            call_id,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ExecCommandRequest {
    pub prepared: PreparedExecCommand,
    pub process_id: i32,
    pub network: Option<NetworkProxy>,
}

/// Target backing a resolved terminal session ID.
#[derive(Clone, Debug)]
pub(crate) enum TerminalTarget {
    Live(Arc<UnifiedExecProcess>),
    Retained { interaction_lock: Arc<Mutex<()>> },
}

/// The process or retained output and environment behind a terminal session ID at lookup time.
#[derive(Clone, Debug)]
pub(crate) struct TerminalSession {
    pub(crate) process_id: i32,
    pub(crate) target: TerminalTarget,
    call_id: String,
    pub(crate) environment_id: String,
}

#[derive(Debug)]
pub(crate) struct WriteStdinRequest<'a> {
    pub terminal: &'a TerminalSession,
    pub input: &'a str,
    pub yield_time_ms: u64,
    pub max_output_tokens: Option<usize>,
    pub truncation_policy: TruncationPolicy,
    pub interaction_event: Option<WriteStdinInteractionEvent<'a>>,
}

pub(crate) struct WriteStdinInteractionEvent<'a> {
    pub session: &'a Arc<Session>,
    pub turn: &'a Arc<TurnContext>,
}

impl std::fmt::Debug for WriteStdinInteractionEvent<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WriteStdinInteractionEvent")
    }
}

struct RetainedEntry {
    meta: retained::RetainedExecMeta,
    bin_path: PathBuf,
    bytes: Option<Arc<Vec<u8>>>,
    interaction_lock: Arc<Mutex<()>>,
    /// True when the process exited asynchronously and the caller has not yet
    /// observed the exit (so the next `write_stdin` emits `PostToolUse` once).
    post_tool_use_pending: bool,
}

#[derive(Default)]
pub(crate) struct ProcessStore {
    processes: HashMap<i32, ProcessEntry>,
    retained: HashMap<i32, RetainedEntry>,
    reserved_process_ids: HashSet<i32>,
}

impl ProcessStore {
    fn remove(&mut self, process_id: i32) -> Option<ProcessEntry> {
        let removed = self.processes.remove(&process_id);
        let keep_reserved = self.retained.contains_key(&process_id)
            || removed.as_ref().is_some_and(|entry| entry.persisted);
        if !keep_reserved && !process_manager::should_use_deterministic_process_ids() {
            self.reserved_process_ids.remove(&process_id);
        }
        removed
    }

    fn transition_live_to_retained(
        &mut self,
        process_id: i32,
        expected_process: Option<&Arc<UnifiedExecProcess>>,
        retained: RetainedEntry,
    ) -> Option<ProcessEntry> {
        let removed = match expected_process {
            Some(expected)
                if self
                    .processes
                    .get(&process_id)
                    .is_some_and(|entry| Arc::ptr_eq(&entry.process, expected)) =>
            {
                self.processes.remove(&process_id)
            }
            Some(_) => None,
            None => self.processes.remove(&process_id),
        };
        self.reserved_process_ids.insert(process_id);
        self.retained.insert(process_id, retained);
        removed
    }
}

pub(crate) struct UnifiedExecProcessManager {
    process_store: Arc<Mutex<ProcessStore>>,
    max_write_stdin_yield_time_ms: u64,
    storage: Option<retained::RetainedExecStorage>,
}

impl UnifiedExecProcessManager {
    pub(crate) fn new(max_write_stdin_yield_time_ms: u64) -> Self {
        Self {
            process_store: Arc::new(Mutex::new(ProcessStore::default())),
            max_write_stdin_yield_time_ms: max_write_stdin_yield_time_ms
                .max(MIN_EMPTY_YIELD_TIME_MS),
            storage: None,
        }
    }

    pub(crate) fn for_thread(
        max_write_stdin_yield_time_ms: u64,
        codex_home: PathBuf,
        thread_id: ThreadId,
        ephemeral: bool,
        forked_from_thread_id: Option<ThreadId>,
    ) -> Self {
        let storage = retained::RetainedExecStorage::new(codex_home, thread_id, ephemeral);
        let mut store = ProcessStore::default();
        let (own_reserved, own_entries) = storage.load_own_entries();
        store.reserved_process_ids.extend(own_reserved);
        for loaded in own_entries {
            store.retained.insert(
                loaded.meta.process_id,
                RetainedEntry {
                    meta: loaded.meta,
                    bin_path: loaded.bin_path,
                    bytes: None,
                    interaction_lock: Arc::new(Mutex::new(())),
                    post_tool_use_pending: false,
                },
            );
        }
        if let Some(source_thread_id) = forked_from_thread_id {
            let (fork_reserved, fork_entries) =
                storage.load_fork_snapshot(source_thread_id, &store.reserved_process_ids);
            store.reserved_process_ids.extend(fork_reserved);
            for loaded in fork_entries {
                store.retained.insert(
                    loaded.meta.process_id,
                    RetainedEntry {
                        meta: loaded.meta,
                        bin_path: loaded.bin_path,
                        bytes: None,
                        interaction_lock: Arc::new(Mutex::new(())),
                        post_tool_use_pending: false,
                    },
                );
            }
        }
        Self {
            process_store: Arc::new(Mutex::new(store)),
            max_write_stdin_yield_time_ms: max_write_stdin_yield_time_ms
                .max(MIN_EMPTY_YIELD_TIME_MS),
            storage: Some(storage),
        }
    }
}

impl Default for UnifiedExecProcessManager {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS)
    }
}

struct ProcessEntry {
    process: Arc<UnifiedExecProcess>,
    plugin_metrics_sidecar: Option<SharedPluginMetricsSidecar>,
    call_id: String,
    process_id: i32,
    cwd: PathUri,
    initial_exec_command_active: Arc<std::sync::atomic::AtomicBool>,
    hook_command: String,
    tty: bool,
    environment_id: String,
    permissions: TerminalPermissions,
    network_approval: Option<DeferredNetworkApproval>,
    session: Weak<Session>,
    last_used: tokio::time::Instant,
    persisted_bytes: usize,
    persisted: bool,
}

type SharedPluginMetricsSidecar = Arc<std::sync::Mutex<Option<PluginMetricsSidecar>>>;

fn take_plugin_metrics_sidecar(
    sidecar: &SharedPluginMetricsSidecar,
) -> Option<PluginMetricsSidecar> {
    sidecar
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

pub(crate) fn clamp_yield_time(yield_time_ms: u64) -> u64 {
    let yield_time_ms = if cfg!(windows) {
        yield_time_ms.max(WINDOWS_INITIAL_EXEC_YIELD_TIME_FLOOR_MS)
    } else {
        yield_time_ms
    };
    yield_time_ms.clamp(MIN_YIELD_TIME_MS, MAX_YIELD_TIME_MS)
}

pub(crate) fn resolve_max_tokens(max_tokens: Option<usize>) -> usize {
    max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
}

pub(crate) fn generate_chunk_id() -> String {
    let mut rng = rng();
    (0..6)
        .map(|_| format!("{:x}", rng.random_range(0..16)))
        .collect()
}

#[cfg(test)]
#[cfg(unix)]
#[path = "process_tests.rs"]
mod process_tests;
#[cfg(test)]
#[cfg(unix)]
#[path = "mod_tests.rs"]
mod tests;
