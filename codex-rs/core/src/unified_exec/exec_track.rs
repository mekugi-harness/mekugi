//! Process-scoped OS shell-write capture bridge for unified exec and user shell.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_change_history::ChangeAuthor;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeSourceKind;
use codex_exec_track::WrappedCommand;
use codex_exec_track::WriteObserverSession;
use codex_exec_track::shell_write_claim_key;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::tools::context::ToolChangeNotice;
use crate::tools::context::tool_change_notice_from_record;

/// Active per-command OS write-observer attachment bound to a single local shell process.
pub(crate) struct ExecTrackAttachment {
    observer: Option<WriteObserverSession>,
    store_dir: PathBuf,
    workspace_root: PathBuf,
    session_tmp: Option<PathBuf>,
    author: ChangeAuthor,
    thread_id: String,
    session_id: String,
    turn_id: String,
    call_id: String,
    source_kind: ChangeSourceKind,
    summary: String,
    settled: OnceLock<()>,
    pending_notice: Mutex<Option<ToolChangeNotice>>,
}

impl ExecTrackAttachment {
    /// Start a per-command process-scoped write observer for an agent shell call.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_for_shell(
        authored_script: &str,
        session_tmp: Option<PathBuf>,
        workspace_root: PathBuf,
        read_policy: Option<(FileSystemSandboxPolicy, PathBuf)>,
        store_dir: PathBuf,
        thread_id: String,
        session_id: String,
        turn_id: String,
        call_id: String,
    ) -> Arc<Self> {
        let author = ChangeAuthor::from_thread_id(&thread_id);
        Self::prepare_with_author(
            authored_script,
            session_tmp,
            workspace_root,
            read_policy,
            store_dir,
            author,
            thread_id,
            session_id,
            turn_id,
            call_id,
            ChangeSourceKind::ShellWrite,
        )
    }

    /// Start a per-command write observer for interactive user-shell (`!`) mode.
    /// Emits a real change ID (`shell1`, `shell2`, ...) attributed to nobody.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_for_user_shell(
        authored_script: &str,
        session_tmp: Option<PathBuf>,
        workspace_root: PathBuf,
        store_dir: PathBuf,
        thread_id: String,
        session_id: String,
        turn_id: String,
        call_id: String,
    ) -> Arc<Self> {
        Self::prepare_with_author(
            authored_script,
            session_tmp,
            workspace_root,
            None,
            store_dir,
            ChangeAuthor::Unattributed,
            thread_id,
            session_id,
            turn_id,
            call_id,
            ChangeSourceKind::UserShell,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_with_author(
        authored_script: &str,
        session_tmp: Option<PathBuf>,
        workspace_root: PathBuf,
        read_policy: Option<(FileSystemSandboxPolicy, PathBuf)>,
        store_dir: PathBuf,
        author: ChangeAuthor,
        thread_id: String,
        session_id: String,
        turn_id: String,
        call_id: String,
        source_kind: ChangeSourceKind,
    ) -> Arc<Self> {
        let observer = match WriteObserverSession::start_with_workspace(
            call_id.clone(),
            Some(workspace_root.clone()),
            move |path| Self::can_read_path(&read_policy, path),
        ) {
            Ok(obs) => Some(obs),
            Err(err) => {
                tracing::debug!("write observer start failed; running unobserved: {err}");
                None
            }
        };

        Arc::new(Self {
            observer,
            store_dir,
            workspace_root,
            session_tmp,
            author,
            thread_id,
            session_id,
            turn_id,
            call_id,
            source_kind,
            summary: authored_script.to_string(),
            settled: OnceLock::new(),
            pending_notice: Mutex::new(None),
        })
    }

    /// Wrap command argv/env so the child process tree reports write syscalls to this observer.
    pub(crate) fn wrap_command(
        &self,
        argv: &[String],
        self_exe: Option<&Path>,
    ) -> Option<WrappedCommand> {
        self.observer.as_ref()?.wrap_command(argv, self_exe)
    }

    /// Filesystem permissions granting sandboxed commands read/write access to
    /// the private per-invocation observer directory (Unix socket / macOS dylib).
    pub(crate) fn additional_permissions(&self) -> Option<AdditionalPermissionProfile> {
        let dir = self.observer.as_ref()?.dir_path();
        let abs = AbsolutePathBuf::from_absolute_path(dir).ok()?;
        Some(AdditionalPermissionProfile {
            file_system: Some(FileSystemPermissions::from_read_write_roots(
                Some(vec![abs.clone()]),
                Some(vec![abs]),
            )),
            ..Default::default()
        })
    }

    /// Stop the observer background thread on early abort or drop.
    pub(crate) fn stop(&self) {
        if let Some(observer) = &self.observer {
            observer.stop();
        }
    }

    /// Settle observation and persist shell-write evidence into ChangeHistoryStore at most once.
    pub(crate) fn settle_once(&self, host_exit_code: Option<i32>) {
        if self.settled.set(()).is_err() {
            return;
        }
        let Some(observer) = &self.observer else {
            return;
        };
        let claim_key = shell_write_claim_key(&self.thread_id, &self.call_id);
        let metadata = ChangeMetadata {
            thread_id: self.thread_id.clone(),
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            call_id: self.call_id.clone(),
            author: Some(self.author.clone()),
            source_kind: self.source_kind,
            workspace_root: self.workspace_root.clone(),
            created_at: unix_now_secs(),
            exit_code: host_exit_code,
            summary: Some(self.summary.clone()),
            claim_key: Some(claim_key),
        };
        match observer.finish_and_persist(
            &self.store_dir,
            &self.workspace_root,
            self.session_tmp.as_deref(),
            metadata,
        ) {
            Ok(Some(record)) => {
                if let Some(notice) = tool_change_notice_from_record(&record) {
                    let mut slot = self
                        .pending_notice
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *slot = Some(notice);
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    call_id = %self.call_id,
                    "failed to persist shell-write change evidence: {err}"
                );
            }
        }
    }

    /// Take the durable change-history notice settled for this command at most once.
    pub(crate) fn take_change_notice(&self) -> Option<ToolChangeNotice> {
        self.pending_notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn can_read_path(policy: &Option<(FileSystemSandboxPolicy, PathBuf)>, path: &Path) -> bool {
        match policy {
            Some((policy, sandbox_cwd)) => policy.can_read_local_path_with_cwd(path, sandbox_cwd),
            None => true,
        }
    }
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[cfg(unix)]
#[path = "exec_track_tests.rs"]
mod tests;
