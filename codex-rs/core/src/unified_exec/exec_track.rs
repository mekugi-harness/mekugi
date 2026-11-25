//! Native exec-tracking and shell-write capture bridge for unified exec and user shell.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeSourceKind;
use codex_exec_server::ExecOutputStream;
#[cfg(unix)]
use codex_exec_track::DrainRequest;
#[cfg(unix)]
use codex_exec_track::ExecTrackReport;
#[cfg(unix)]
use codex_exec_track::ExecTrackSession;
#[cfg(unix)]
use codex_exec_track::OutputStreamKind;
use codex_exec_track::ShellDialect;
use codex_exec_track::ShellWriteCapture;
#[cfg(unix)]
use codex_exec_track::TrackingMode;
use codex_exec_track::shell_write_claim_key;
#[cfg(unix)]
use codex_exec_track::split_segments;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio::sync::mpsc;

use crate::shell::ShellType;

/// Active native exec-tracking and shell-write capture attachment bound to a
/// single local shell process.
pub(crate) struct ExecTrackAttachment {
    #[cfg(unix)]
    session: Option<Arc<ExecTrackSession>>,
    write_capture: ShellWriteCapture,
    store_dir: PathBuf,
    workspace_root: PathBuf,
    read_policy: Option<(FileSystemSandboxPolicy, PathBuf)>,
    thread_id: String,
    session_id: String,
    turn_id: String,
    call_id: String,
    source_kind: ChangeSourceKind,
    summary: String,
    settled: OnceLock<()>,
}

/// Prepared native exec-tracking state returned before process spawn.
pub(crate) struct PreparedExecTrack {
    pub(crate) attachment: Arc<ExecTrackAttachment>,
    /// Instrumented single-shell script when AST eligibility succeeded.
    pub(crate) instrumented_script: Option<String>,
    #[cfg(unix)]
    pub(crate) drain_rx: Option<mpsc::UnboundedReceiver<DrainRequest>>,
}

impl ExecTrackAttachment {
    /// Prepare native exec tracking and pre-execution shell-write baselines for
    /// a local shell invocation on the authored post-PreToolUse script.
    pub(crate) fn prepare_for_shell(
        authored_script: &str,
        cwd: &Path,
        session_tmp: Option<&Path>,
        workspace_root: PathBuf,
        read_policy: Option<(FileSystemSandboxPolicy, PathBuf)>,
        shell_type: &ShellType,
        tty: bool,
        store_dir: PathBuf,
        thread_id: String,
        session_id: String,
        turn_id: String,
        call_id: String,
    ) -> PreparedExecTrack {
        Self::prepare_for_shell_with_source(
            authored_script,
            cwd,
            session_tmp,
            workspace_root,
            read_policy,
            shell_type,
            tty,
            store_dir,
            thread_id,
            session_id,
            turn_id,
            call_id,
            ChangeSourceKind::ShellWrite,
        )
    }

    /// Prepare native exec tracking and pre-execution shell-write baselines with
    /// an explicit [`ChangeSourceKind`] provenance label.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_for_shell_with_source(
        authored_script: &str,
        cwd: &Path,
        session_tmp: Option<&Path>,
        workspace_root: PathBuf,
        read_policy: Option<(FileSystemSandboxPolicy, PathBuf)>,
        shell_type: &ShellType,
        tty: bool,
        store_dir: PathBuf,
        thread_id: String,
        session_id: String,
        turn_id: String,
        call_id: String,
        source_kind: ChangeSourceKind,
    ) -> PreparedExecTrack {
        let write_capture =
            ShellWriteCapture::prepare_with_policy(authored_script, cwd, session_tmp, |path| {
                Self::can_read_path(&read_policy, path)
            });

        #[cfg(unix)]
        let (session, instrumented_script, drain_rx) = {
            let dialect = match shell_type {
                ShellType::Bash => Some(ShellDialect::Bash),
                ShellType::Sh if cfg!(target_os = "linux") => Some(ShellDialect::Dash),
                _ => None,
            };
            if let Some(dialect) = dialect
                && let Some(split) = split_segments(authored_script, dialect)
            {
                let mode = if tty {
                    TrackingMode::Tty
                } else {
                    TrackingMode::Pipe
                };
                match ExecTrackSession::start(split.segments.clone(), mode) {
                    Ok((session, drain_rx)) => {
                        let instrumented =
                            session.instrument_script(authored_script, &split.segments, dialect);
                        (Some(Arc::new(session)), Some(instrumented), Some(drain_rx))
                    }
                    Err(err) => {
                        tracing::debug!("exec_track session start failed; falling back: {err}");
                        (None, None, None)
                    }
                }
            } else {
                (None, None, None)
            }
        };

        #[cfg(not(unix))]
        let instrumented_script: Option<String> = {
            let _ = (shell_type, tty);
            None
        };

        let attachment = Arc::new(Self {
            #[cfg(unix)]
            session,
            write_capture,
            store_dir,
            workspace_root,
            read_policy,
            thread_id,
            session_id,
            turn_id,
            call_id,
            source_kind,
            summary: authored_script.to_string(),
            settled: OnceLock::new(),
        });

        PreparedExecTrack {
            attachment,
            instrumented_script,
            #[cfg(unix)]
            drain_rx,
        }
    }

    /// Filesystem permissions granting sandboxed commands read/write access to
    /// the private per-invocation tracker directory.
    pub(crate) fn additional_permissions(&self) -> Option<AdditionalPermissionProfile> {
        #[cfg(unix)]
        {
            let dir = self.session.as_ref()?.dir_path();
            let abs = AbsolutePathBuf::from_absolute_path(dir).ok()?;
            Some(AdditionalPermissionProfile {
                file_system: Some(FileSystemPermissions::from_read_write_roots(
                    Some(vec![abs.clone()]),
                    Some(vec![abs]),
                )),
                ..Default::default()
            })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Forward output bytes from the native output collector into the active
    /// tracked segment.
    pub(crate) fn push_output(&self, stream: ExecOutputStream, bytes: &[u8]) {
        #[cfg(unix)]
        if let Some(session) = &self.session {
            let kind = match stream {
                ExecOutputStream::Stdout | ExecOutputStream::Pty => OutputStreamKind::Stdout,
                ExecOutputStream::Stderr => OutputStreamKind::Stderr,
            };
            session.push_output(kind, bytes);
        }
        #[cfg(not(unix))]
        let _ = (stream, bytes);
    }

    /// Abort owned tracker tasks and unlink FIFOs on early stop or drop.
    pub(crate) fn stop(&self) {
        #[cfg(unix)]
        if let Some(session) = &self.session {
            session.stop();
        }
    }

    /// Settle tracking and persist shell-write / mktemp evidence into
    /// ChangeHistoryStore at most once.
    pub(crate) fn settle_once(&self, host_exit_code: Option<i32>) {
        if self.settled.set(()).is_err() {
            return;
        }
        #[cfg(unix)]
        let (mktemp_creations, _report): (
            Vec<codex_exec_track::MktempCreation>,
            Option<ExecTrackReport>,
        ) = if let Some(session) = &self.session {
            let report = session.finalize(host_exit_code);
            if let Some(reason) = &report.fallback_reason {
                tracing::debug!(
                    call_id = %self.call_id,
                    "exec_track fell back to stock combined output: {reason}"
                );
            }
            let mk = report.mktemp_creations.clone();
            (mk, Some(report))
        } else {
            (Vec::new(), None)
        };

        #[cfg(unix)]
        {
            if self.write_capture.candidate_count() == 0 && mktemp_creations.is_empty() {
                return;
            }
            let source_kind = if self.source_kind == ChangeSourceKind::ShellWrite
                && self.write_capture.is_vcs_only()
                && mktemp_creations.is_empty()
            {
                ChangeSourceKind::VcsDiagnostic
            } else {
                self.source_kind
            };
            let claim_key = shell_write_claim_key(&self.thread_id, &self.call_id);
            let metadata = ChangeMetadata {
                thread_id: self.thread_id.clone(),
                session_id: self.session_id.clone(),
                turn_id: self.turn_id.clone(),
                call_id: self.call_id.clone(),
                source_kind,
                workspace_root: self.workspace_root.clone(),
                created_at: unix_now_secs(),
                exit_code: host_exit_code,
                summary: Some(self.summary.clone()),
                claim_key: Some(claim_key),
            };
            if let Err(err) = self.write_capture.finalize_and_persist_with_policy(
                &self.store_dir,
                metadata,
                &mktemp_creations,
                |path| Self::can_read_path(&self.read_policy, path),
            ) {
                tracing::warn!(
                    call_id = %self.call_id,
                    "failed to persist native shell-write change evidence: {err}"
                );
            }
        }
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
