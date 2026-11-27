//! Core-owned queue metadata; forwarding transfers the residency guard with the operation.

use codex_extension_api::ExtensionDataInit;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::W3cTraceContext;
use codex_protocol::turn_input::StartIfIdleSubmission;
use codex_protocol::turn_input::TurnInputMode;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::turn_input::TurnInputSubmission;
use codex_user_input::ResolvedUserInput;
use tokio::sync::OwnedRwLockReadGuard;
use tokio::sync::oneshot;

/// Turn input crosses this queue only after the submitting host has resolved its attachments.
pub(crate) enum SubmissionOp {
    Protocol(Box<Op>),
    InterruptTurn {
        turn_id: String,
    },
    TurnInput {
        request: Box<TurnInputRequest<ResolvedUserInput>>,
        mode: TurnInputMode,
        reply: oneshot::Sender<CodexResult<TurnInputSubmission>>,
    },
    ShellIfIdle {
        command: String,
        reply: oneshot::Sender<StartIfIdleSubmission>,
    },
    Compact {
        admission: CompactAdmission,
        reply: oneshot::Sender<StartIfIdleSubmission>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CompactAdmission {
    ReplaceAndContinue,
    IfIdle,
}

impl From<Op> for SubmissionOp {
    fn from(op: Op) -> Self {
        Self::Protocol(Box::new(op))
    }
}

impl SubmissionOp {
    pub(crate) fn as_protocol(&self) -> Option<&Op> {
        match self {
            Self::Protocol(op) => Some(op),
            Self::InterruptTurn { .. }
            | Self::TurnInput { .. }
            | Self::ShellIfIdle { .. }
            | Self::Compact { .. } => None,
        }
    }

    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Protocol(op) => op.kind(),
            Self::InterruptTurn { .. } => "interrupt",
            Self::TurnInput { .. } => "turn_input",
            Self::ShellIfIdle { .. } => "shell_if_idle",
            Self::Compact { .. } => "manual_compact",
        }
    }
}

impl std::fmt::Debug for SubmissionOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(op) => std::fmt::Debug::fmt(op, f),
            Self::InterruptTurn { turn_id } => {
                f.debug_tuple("InterruptTurn").field(turn_id).finish()
            }
            Self::TurnInput { request, .. } => f
                .debug_struct("TurnInput")
                .field("requested_settings", &request.thread_settings)
                .finish(),
            Self::ShellIfIdle { .. } => f.debug_struct("ShellIfIdle").finish_non_exhaustive(),
            Self::Compact { admission, .. } => f.debug_tuple("Compact").field(admission).finish(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Submission {
    pub id: String,
    pub op: SubmissionOp,
    /// Accepted with settings, never separately from the request that supplied them.
    pub turn_extension_init: Option<ExtensionDataInit>,
    /// Optional W3C trace carrier propagated across async submission handoffs.
    pub trace: Option<W3cTraceContext>,
    pub parent_turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    /// Keeps a V2 recipient resident until this submission is handled or dropped.
    pub residency_guard: Option<OwnedRwLockReadGuard<()>>,
    /// Owns delivery until dispatch; failed or abandoned submissions discard it.
    pub message_delivery: Option<crate::agent_communication::MessageDelivery>,
}
