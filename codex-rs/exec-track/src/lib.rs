//! Process-scoped OS write observer for Mekugi shell execution.
//!
//! Replaces Bash AST write-path heuristics and shell script rewriting with
//! OS-level per-process write interception:
//! - Linux: `seccomp-unotify` (`SECCOMP_RET_USER_NOTIF` + `SECCOMP_USER_NOTIF_FLAG_CONTINUE`).
//! - macOS: `DYLD_INSERT_LIBRARIES` (`__DATA,__interpose`) + non-SIP shell preference.
//!
//! Each shell command gets its own isolated observer tied to its `(thread_id, call_id)`,
//! works on non-git directories, captures writes outside cwd, and classifies `/tmp`
//! (`ChangeScope::Ephemeral`) separately from workspace/external edits.

mod active_writers;
mod ipc;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod persist;

use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use codex_change_history::ChangeHistoryError;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeRecord;
use tempfile::TempDir;

#[cfg(target_os = "linux")]
pub use linux::run_observe_wrapper_main;
#[cfg(target_os = "macos")]
pub use macos::run_observe_wrapper_main;
use persist::ObservationState;
pub use persist::live::LiveFileChange;
pub use persist::live::LiveFileChangeKind;

/// Internal arg1 used by `codex-arg0` to dispatch the Linux `seccomp-unotify` child wrapper.
pub const CODEX_EXEC_OBSERVE_ARG1: &str = "--codex-exec-observe";

/// Compute the shared deduplication `claim_key` for `(thread_id, call_id)` so
/// overlapping `apply_patch` and shell observers claim the same logical call at most once.
pub fn shell_write_claim_key(thread_id: &str, call_id: &str) -> String {
    format!("{thread_id}:{call_id}")
}

/// Command argv and environment adjustments produced by [`WriteObserverSession::wrap_command`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrappedCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Active per-command process-scoped write observer.
pub struct WriteObserverSession {
    dir: TempDir,
    state: Arc<Mutex<ObservationState>>,
    #[cfg(target_os = "linux")]
    backend: linux::LinuxObserver,
    #[cfg(target_os = "macos")]
    backend: macos::MacosObserver,
}

impl WriteObserverSession {
    /// Start a new per-command write observer with a sandbox read-policy predicate.
    pub fn start(
        call_id: String,
        can_read: impl Fn(&Path) -> bool + Send + Sync + 'static,
    ) -> io::Result<Self> {
        Self::start_with_workspace(call_id, None, can_read)
    }

    /// Start a new per-command write observer with an explicit workspace root
    /// (which takes precedence over non-authored toolchain cache ignores).
    pub fn start_with_workspace(
        call_id: String,
        workspace_root: Option<PathBuf>,
        can_read: impl Fn(&Path) -> bool + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let dir = tempfile::Builder::new().prefix("mekugi-obs-").tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700));
        }
        let state = Arc::new(Mutex::new(ObservationState::new_with_workspace(
            call_id,
            dir.path().to_path_buf(),
            workspace_root,
            Arc::new(can_read),
        )));
        #[cfg(target_os = "linux")]
        let backend = linux::LinuxObserver::start(dir.path(), Arc::clone(&state))?;
        #[cfg(target_os = "macos")]
        let backend = macos::MacosObserver::start(dir.path(), Arc::clone(&state))?;
        Ok(Self {
            dir,
            state,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            backend,
        })
    }

    /// Private directory holding the observer's Unix socket (and macOS dylib),
    /// granted read/write access in sandboxed commands.
    pub fn dir_path(&self) -> &Path {
        self.dir.path()
    }

    /// Wrap a shell command so its child process tree reports write syscalls to this observer.
    pub fn wrap_command(&self, argv: &[String], self_exe: Option<&Path>) -> Option<WrappedCommand> {
        #[cfg(target_os = "linux")]
        {
            self.backend.wrap_command(argv, self_exe)
        }
        #[cfg(target_os = "macos")]
        {
            self.backend.wrap_command(argv, self_exe)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (argv, self_exe);
            None
        }
    }

    /// Check for observed paths without waiting for the write supervisor or reading files.
    pub fn has_live_candidates(&self) -> bool {
        self.state
            .try_lock()
            .is_ok_and(|state| state.has_live_candidates())
    }

    /// Sample current authored edits without settling or persisting the command.
    /// Call from blocking work: the snapshot reads the filesystem, but not while
    /// holding the write supervisor's lock.
    pub fn live_changes(
        &self,
        workspace_root: &Path,
        session_tmp: Option<&Path>,
    ) -> Vec<LiveFileChange> {
        let baselines = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .live_baselines();
        baselines.render(workspace_root, session_tmp)
    }

    /// Stop observing new writes without discarding already-captured baselines,
    /// so callers (e.g. timed-out command termination) can still call
    /// `finish_and_persist` afterwards.
    pub fn stop(&self) {
        #[cfg(target_os = "linux")]
        self.backend.settle();
        #[cfg(target_os = "macos")]
        self.backend.stop_and_join();
    }

    /// Stop the observer, read post-execution snapshots for all observed paths,
    /// and persist durable evidence into `ChangeHistoryStore`.
    pub fn finish_and_persist(
        &self,
        store_dir: &Path,
        workspace_root: &Path,
        session_tmp: Option<&Path>,
        metadata: ChangeMetadata,
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        self.stop();
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.finalize_and_persist(store_dir, workspace_root, session_tmp, metadata)
    }
}

impl Drop for WriteObserverSession {
    fn drop(&mut self) {
        self.stop();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release_active_writers();
    }
}

#[cfg(test)]
#[path = "exec_track_tests.rs"]
mod tests;
