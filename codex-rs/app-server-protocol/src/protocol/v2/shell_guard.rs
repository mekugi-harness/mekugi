use crate::JsonSchema;
use crate::TS;
use codex_protocol::shell_guard::ShellGuardApprovalDecision as CoreShellGuardApprovalDecision;
use codex_protocol::shell_guard::ShellGuardApprovalKind as CoreShellGuardApprovalKind;
use codex_protocol::shell_guard::ShellGuardSecretDecision as CoreShellGuardSecretDecision;
use codex_protocol::shell_guard::ShellGuardSecretKind as CoreShellGuardSecretKind;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use zeroize::Zeroize;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ShellGuardApprovalKind {
    RemoteWrite,
    Sudo,
}

impl From<CoreShellGuardApprovalKind> for ShellGuardApprovalKind {
    fn from(kind: CoreShellGuardApprovalKind) -> Self {
        match kind {
            CoreShellGuardApprovalKind::RemoteWrite => Self::RemoteWrite,
            CoreShellGuardApprovalKind::Sudo => Self::Sudo,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ShellGuardRequestApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub item_id: Option<String>,
    #[ts(type = "number")]
    pub started_at_ms: i64,
    pub kind: ShellGuardApprovalKind,
    pub cwd: String,
    pub executable: String,
    pub argv: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ShellGuardApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
}

impl From<ShellGuardApprovalDecision> for CoreShellGuardApprovalDecision {
    fn from(decision: ShellGuardApprovalDecision) -> Self {
        match decision {
            ShellGuardApprovalDecision::Accept => Self::Accept,
            ShellGuardApprovalDecision::AcceptForSession => Self::AcceptForSession,
            ShellGuardApprovalDecision::Decline => Self::Decline,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ShellGuardRequestApprovalResponse {
    pub decision: ShellGuardApprovalDecision,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ShellGuardSecretKind {
    SudoPassword,
    GpgPassphrase,
}

impl From<CoreShellGuardSecretKind> for ShellGuardSecretKind {
    fn from(kind: CoreShellGuardSecretKind) -> Self {
        match kind {
            CoreShellGuardSecretKind::SudoPassword => Self::SudoPassword,
            CoreShellGuardSecretKind::GpgPassphrase => Self::GpgPassphrase,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ShellGuardRequestSecretParams {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub item_id: Option<String>,
    #[ts(type = "number")]
    pub started_at_ms: i64,
    pub kind: ShellGuardSecretKind,
    pub cwd: String,
    pub executable: String,
    pub argv: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ShellGuardSecretDecision {
    Provide,
    Decline,
}

impl From<ShellGuardSecretDecision> for CoreShellGuardSecretDecision {
    fn from(decision: ShellGuardSecretDecision) -> Self {
        match decision {
            ShellGuardSecretDecision::Provide => Self::Provide,
            ShellGuardSecretDecision::Decline => Self::Decline,
        }
    }
}

/// Client response for `shellGuard/requestSecret`. Redacts `secret` from `Debug`
/// output and zeroes its memory on drop.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ShellGuardRequestSecretResponse {
    pub decision: ShellGuardSecretDecision,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl Drop for ShellGuardRequestSecretResponse {
    fn drop(&mut self) {
        if let Some(secret) = self.secret.as_mut() {
            secret.zeroize();
        }
    }
}

impl fmt::Debug for ShellGuardRequestSecretResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShellGuardRequestSecretResponse")
            .field("decision", &self.decision)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("reason", &self.reason)
            .finish()
    }
}
