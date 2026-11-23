use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_features::Feature;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SessionSource;
use codex_protocol::shell_guard::ShellGuardApprovalDecision;
use codex_protocol::shell_guard::ShellGuardApprovalKind;
use codex_protocol::shell_guard::ShellGuardApprovalRequestEvent;
use codex_protocol::shell_guard::ShellGuardRequestResolvedReason;
use codex_protocol::shell_guard::ShellGuardSecretDecision;
use codex_protocol::shell_guard::ShellGuardSecretKind;
use codex_protocol::shell_guard::ShellGuardSecretRequestEvent;
use codex_protocol::shell_guard::ShellGuardSecretValue;
use codex_shell_guard::GuardKind;
use codex_shell_guard::SecretString;
use codex_shell_guard::ShellGuardBroker;
use codex_shell_guard::ShellGuardBrokerConfig;
use codex_shell_guard::ShellGuardBrokerDecision;
use codex_shell_guard::ShellGuardHandler;
use codex_shell_guard::ShellGuardResolvedReason;
use codex_shell_guard::ShimRequest;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::Mutex;
use tokio::sync::oneshot;

pub(crate) fn active(turn: &TurnContext, environment_is_remote: bool) -> bool {
    cfg!(unix)
        && !environment_is_remote
        && turn.config.features.get().enabled(Feature::ShellGuard)
        && !matches!(
            turn.session_source,
            SessionSource::Exec | SessionSource::Internal(_)
        )
}

/// The aliases are staged together by arg0 before the runtime starts. Keep
/// their pinned executable identity for absolute and nested shell rewrites too.
pub(crate) struct GuardLaunch {
    pub(crate) socket: PathBuf,
    pub(crate) vcs_dir: PathBuf,
    pub(crate) sudo_dir: PathBuf,
    helper: PathBuf,
}

impl GuardLaunch {
    pub(crate) async fn prepare(
        session: &Arc<Session>,
        alias_dir: Option<&Path>,
    ) -> io::Result<Self> {
        let alias_dir =
            alias_dir.ok_or_else(|| io::Error::other("shell guard aliases unavailable"))?;
        Ok(Self {
            socket: session
                .services
                .shell_guard
                .ensure_socket(session, alias_dir)
                .await?,
            vcs_dir: alias_dir.join(codex_shell_guard::VCS_DIR_NAME),
            sudo_dir: alias_dir.join(codex_shell_guard::SUDO_DIR_NAME),
            helper: alias_dir.join(codex_shell_guard::PINNED_EXE_NAME),
        })
    }

    pub(crate) fn set_environment(&self, env: &mut HashMap<String, String>) {
        for (key, path) in [
            (codex_shell_guard::SOCKET_ENV_VAR, &self.socket),
            (codex_shell_guard::VCS_DIR_ENV_VAR, &self.vcs_dir),
            (codex_shell_guard::SUDO_DIR_ENV_VAR, &self.sudo_dir),
        ] {
            env.insert(key.to_owned(), path.to_string_lossy().into_owned());
        }
    }

    pub(crate) fn rewrite(
        &self,
        script: &str,
        thread_id: &str,
        item_id: &str,
        login: bool,
    ) -> String {
        let script = codex_shell_guard::rewrite_script(
            script,
            &codex_shell_guard::RewriteConfig {
                helper_exe: &self.helper,
                socket_path: &self.socket,
                vcs_dir: Some(&self.vcs_dir),
                sudo_dir: Some(&self.sudo_dir),
                thread_id,
                item_id,
                force_rewrite_bare: false,
            },
        );
        if !login {
            return script;
        }
        // Login startup may put system binaries before our inherited aliases.
        // Restore precedence inside the chosen shell, after startup/snapshot,
        // including when the user configured a PATH of their own.
        let dirs = codex_shell_guard::protocol::shell_single_quote(&format!(
            "{}:{}",
            self.vcs_dir.display(),
            self.sudo_dir.display()
        ));
        format!(
            "case \"$PATH:\" in {dirs}:*) ;; *) export PATH={dirs}${{PATH:+:\"$PATH\"}} || exit 1 ;; esac; {script}"
        )
    }
}

struct PendingShellGuardEntry {
    sender: oneshot::Sender<ShellGuardBrokerDecision>,
    is_secret: bool,
}

/// Session-scoped owner of the local Unix socket broker and pending shell-guard
/// approval/secret channels.
#[derive(Default)]
pub(crate) struct ShellGuardService {
    broker: Mutex<Option<ShellGuardBroker>>,
    pending: Arc<Mutex<HashMap<String, PendingShellGuardEntry>>>,
    next_id: Arc<AtomicU64>,
}

struct SessionShellGuardRouter {
    session: Weak<Session>,
    pending: Arc<Mutex<HashMap<String, PendingShellGuardEntry>>>,
    next_id: Arc<AtomicU64>,
}

impl ShellGuardHandler for SessionShellGuardRouter {
    async fn handle_request(
        &self,
        request: ShimRequest,
    ) -> Option<(String, oneshot::Receiver<ShellGuardBrokerDecision>)> {
        let session = self.session.upgrade()?;
        let turn_id = session.last_started_turn_id().await;
        let request_id = format!("sg-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        let item_id = (!request.item_id.is_empty()).then_some(request.item_id);
        let is_secret = request.kind.is_secret();

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(
            request_id.clone(),
            PendingShellGuardEntry {
                sender: tx,
                is_secret,
            },
        );

        let msg = match request.kind {
            GuardKind::RemoteWrite => {
                EventMsg::ShellGuardApprovalRequest(ShellGuardApprovalRequestEvent {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id,
                    started_at_ms,
                    kind: ShellGuardApprovalKind::RemoteWrite,
                    cwd: request.cwd,
                    executable: request.executable,
                    argv: request.argv,
                })
            }
            GuardKind::Sudo => {
                EventMsg::ShellGuardApprovalRequest(ShellGuardApprovalRequestEvent {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id,
                    started_at_ms,
                    kind: ShellGuardApprovalKind::Sudo,
                    cwd: request.cwd,
                    executable: request.executable,
                    argv: request.argv,
                })
            }
            GuardKind::SudoPassword => {
                EventMsg::ShellGuardSecretRequest(ShellGuardSecretRequestEvent {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id,
                    started_at_ms,
                    kind: ShellGuardSecretKind::SudoPassword,
                    cwd: request.cwd,
                    executable: request.executable,
                    argv: request.argv,
                })
            }
            GuardKind::GpgPassphrase => {
                EventMsg::ShellGuardSecretRequest(ShellGuardSecretRequestEvent {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id,
                    started_at_ms,
                    kind: ShellGuardSecretKind::GpgPassphrase,
                    cwd: request.cwd,
                    executable: request.executable,
                    argv: request.argv,
                })
            }
        };

        session
            .send_event_raw_without_materializing_rollout(Event { id: turn_id, msg })
            .await;

        Some((request_id, rx))
    }

    async fn handle_resolved(&self, request_id: String, reason: ShellGuardResolvedReason) {
        self.pending.lock().await.remove(&request_id);
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let turn_id = session.last_started_turn_id().await;
        let mapped = match reason {
            ShellGuardResolvedReason::Withdrawn => ShellGuardRequestResolvedReason::Withdrawn,
            ShellGuardResolvedReason::TimedOut => ShellGuardRequestResolvedReason::TimedOut,
        };
        session
            .send_event_raw_without_materializing_rollout(Event {
                id: turn_id,
                msg: EventMsg::ShellGuardRequestResolved {
                    request_id,
                    reason: mapped,
                },
            })
            .await;
    }
}

impl ShellGuardService {
    /// Lazily starts the per-session local Unix socket broker and returns its path.
    #[tracing::instrument(level = "debug", skip(self, session))]
    async fn ensure_socket(&self, session: &Arc<Session>, alias_dir: &Path) -> io::Result<PathBuf> {
        let mut guard = self.broker.lock().await;
        if let Some(broker) = guard.as_ref() {
            return Ok(broker.socket_path().to_path_buf());
        }
        #[cfg(unix)]
        {
            let alias_dir = alias_dir.to_path_buf();
            tokio::task::spawn_blocking(move || codex_shell_guard::stage_aliases(&alias_dir))
                .await
                .map_err(io::Error::other)??;
        }
        let router = Arc::new(SessionShellGuardRouter {
            session: Arc::downgrade(session),
            pending: Arc::clone(&self.pending),
            next_id: Arc::clone(&self.next_id),
        });
        let broker = ShellGuardBroker::start(router, ShellGuardBrokerConfig::default()).await?;
        let socket_path = broker.socket_path().to_path_buf();
        *guard = Some(broker);
        Ok(socket_path)
    }

    pub(crate) async fn resolve_approval(
        &self,
        id: &str,
        decision: ShellGuardApprovalDecision,
        reason: Option<String>,
    ) {
        let Some(entry) = self.pending.lock().await.remove(id) else {
            return;
        };
        if entry.is_secret {
            let _ = entry.sender.send(ShellGuardBrokerDecision::Deny(
                "invalid response type".to_owned(),
            ));
            return;
        }
        let broker_decision = match decision {
            ShellGuardApprovalDecision::Accept | ShellGuardApprovalDecision::AcceptForSession => {
                ShellGuardBrokerDecision::Allow
            }
            ShellGuardApprovalDecision::Decline => ShellGuardBrokerDecision::Deny(
                reason.unwrap_or_else(|| "user denied this command without a reason".to_owned()),
            ),
        };
        let _ = entry.sender.send(broker_decision);
    }

    pub(crate) async fn resolve_secret(
        &self,
        id: &str,
        decision: ShellGuardSecretDecision,
        secret: Option<ShellGuardSecretValue>,
        reason: Option<String>,
    ) {
        let Some(entry) = self.pending.lock().await.remove(id) else {
            return;
        };
        if !entry.is_secret {
            let _ = entry.sender.send(ShellGuardBrokerDecision::Deny(
                "invalid response type".to_owned(),
            ));
            return;
        }
        let broker_decision = match (decision, secret) {
            (ShellGuardSecretDecision::Provide, Some(secret)) => {
                ShellGuardBrokerDecision::ProvideSecret(SecretString::new(secret.into_inner()))
            }
            _ => ShellGuardBrokerDecision::Deny(
                reason.unwrap_or_else(|| "authentication denied".to_owned()),
            ),
        };
        let _ = entry.sender.send(broker_decision);
    }

    pub(crate) async fn shutdown(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (_, entry) in pending {
            let _ = entry
                .sender
                .send(ShellGuardBrokerDecision::Deny("session closed".to_owned()));
        }
        let broker = self.broker.lock().await.take();
        if let Some(broker) = broker {
            broker.shutdown().await;
        }
    }
}
