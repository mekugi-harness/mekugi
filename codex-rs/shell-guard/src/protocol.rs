use crate::secret::SecretString;
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;

/// Server-owned approval/secret decision timeout (5 minutes).
pub const TIMEOUT: Duration = Duration::from_secs(300);

/// Client socket deadline exceeds server timeout so a server denial arrives
/// before the shim closes and looks like a withdrawn command.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(305);

/// Maximum wire message size (1 MiB).
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// Maximum argv metadata budget embedded in `CODEX_SHELL_GUARD_SUDO_COMMAND`.
pub const SUDO_ARGV_DISPLAY_BUDGET: usize = 4096;

/// Subdirectory names staged beneath the per-session arg0 alias directory.
pub const PINNED_EXE_NAME: &str = ".codex-self";
pub const VCS_DIR_NAME: &str = "vcs-guard";
pub const SUDO_DIR_NAME: &str = "sudo-guard";
pub const ASKPASS_NAME: &str = "mekugi-sudo-askpass";

/// Tools staged in `vcs-guard/`.
pub const VCS_TOOLS: &[&str] = &["git", "gh", "hg", "svn", "jj"];

/// Tools staged in `sudo-guard/` (excluding `ASKPASS_NAME`, which is also staged there).
pub const SUDO_TOOLS: &[&str] = &["sudo", "gpg", "gpg2"];

/// Nested POSIX shells recognized by the minimal AST rewriter.
pub const SHELLS: &[&str] = &["sh", "bash", "dash", "ksh", "zsh"];

/// Environment variables carried into guarded child processes.
pub const SOCKET_ENV_VAR: &str = "CODEX_SHELL_GUARD_SOCKET";
pub const SUDO_DIR_ENV_VAR: &str = "CODEX_SHELL_GUARD_SUDO_DIR";
pub const VCS_DIR_ENV_VAR: &str = "CODEX_SHELL_GUARD_VCS_DIR";
pub const SUDO_COMMAND_ENV_VAR: &str = "CODEX_SHELL_GUARD_SUDO_COMMAND";
pub const THREAD_ID_ENV_VAR: &str = "CODEX_THREAD_ID";
pub const ITEM_ID_ENV_VAR: &str = "CODEX_TOOL_CALL_ID";

/// Internal `argv[1]` flags used by the minimal AST rewriter and GPG supervisor.
pub const ARG1_VCS_COMMAND: &str = "--codex-shell-guard-vcs";
pub const ARG1_VCS_COMMAND_DEFAULT: &str = "--codex-shell-guard-vcs-default";
pub const ARG1_SUDO_COMMAND: &str = "--codex-shell-guard-sudo";
pub const ARG1_SUDO_COMMAND_DEFAULT: &str = "--codex-shell-guard-sudo-default";
pub const ARG1_GPG_COMMAND: &str = "--codex-shell-guard-gpg";
pub const ARG1_GPG_COMMAND_DEFAULT: &str = "--codex-shell-guard-gpg-default";
pub const ARG1_GPG_RUN: &str = "--codex-shell-guard-gpg-run";
pub const ARG1_SHELL: &str = "--codex-shell-guard-shell";
pub const ARG1_SHELL_DEFAULT: &str = "--codex-shell-guard-shell-default";

/// Classification of a shim request sent over the session-private Unix socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuardKind {
    RemoteWrite,
    Sudo,
    SudoPassword,
    GpgPassphrase,
}

impl GuardKind {
    pub fn is_secret(self) -> bool {
        matches!(self, Self::SudoPassword | Self::GpgPassphrase)
    }
}

/// Request sent by a guard shim to the session's `ShellGuardBroker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShimRequest {
    pub kind: GuardKind,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub cwd: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub executable: String,
}

/// Reply returned by the `ShellGuardBroker` to the waiting guard shim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShimReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretString>,
}

impl ShimReply {
    pub fn allow() -> Self {
        Self {
            ok: true,
            reason: None,
            secret: None,
        }
    }

    pub fn provide_secret(secret: SecretString) -> Self {
        Self {
            ok: true,
            reason: None,
            secret: Some(secret),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            reason: Some(reason.into()),
            secret: None,
        }
    }
}

/// Shell single-quoting helper shared by the rewriter and git alias script builder.
pub fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}
