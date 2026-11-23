use std::fmt;
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGuardApprovalKind {
    RemoteWrite,
    Sudo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGuardSecretKind {
    SudoPassword,
    GpgPassphrase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGuardApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGuardSecretDecision {
    Provide,
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGuardRequestResolvedReason {
    Withdrawn,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellGuardApprovalRequestEvent {
    pub request_id: String,
    pub turn_id: String,
    pub item_id: Option<String>,
    pub started_at_ms: i64,
    pub kind: ShellGuardApprovalKind,
    pub cwd: String,
    pub executable: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellGuardSecretRequestEvent {
    pub request_id: String,
    pub turn_id: String,
    pub item_id: Option<String>,
    pub started_at_ms: i64,
    pub kind: ShellGuardSecretKind,
    pub cwd: String,
    pub executable: String,
    pub argv: Vec<String>,
}

/// Redacted, zeroized secret passed from app-server to codex-core for askpass / GPG.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ShellGuardSecretValue(String);

impl ShellGuardSecretValue {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn into_inner(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl Drop for ShellGuardSecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for ShellGuardSecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
