//! Direct-message diagnostics and delivery ownership across queues and eviction.
//! Only history commitment consumes mail; dropping its last owner releases its attachments.

use codex_extension_api::ExtensionData;
use codex_extension_api::MessageLifecycleContributor;
use codex_extension_api::MessageLifecycleInput;
use codex_protocol::ThreadId;
use codex_protocol::protocol::InterAgentCommunication;
use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

pub(crate) static PENDING_MAILBOX_MESSAGES: codex_diagnostics::Gauge =
    codex_diagnostics::Gauge::new("core.mailbox.pending");

const AGENT_COMMUNICATION_TARGET: &str = "codex_otel.agent_communication";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentCommunicationKind {
    Spawn,
    Message,
    Followup,
    Result,
}

impl AgentCommunicationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Message => "message",
            Self::Followup => "followup",
            Self::Result => "result",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentCommunicationContext {
    kind: AgentCommunicationKind,
    pub(crate) sender_thread_id: ThreadId,
    pub(crate) final_capture: Option<Arc<FinalCaptureAttachment>>,
    pub(crate) result_operation: Option<String>,
}

/// Retained selection identity follows native mail until the recipient consumes it.
/// Projection happens then, against that recipient's current context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FinalCaptureAttachment {
    pub(crate) root_thread_id: ThreadId,
    pub(crate) sender_thread_id: ThreadId,
    pub(crate) operation_id: String,
    pub(crate) recipient_cwd: Option<String>,
    pub(crate) resource_ref: String,
    pub(crate) full_text: String,
}

impl AgentCommunicationContext {
    pub(crate) fn new(kind: AgentCommunicationKind, sender_thread_id: ThreadId) -> Self {
        Self {
            kind,
            sender_thread_id,
            final_capture: None,
            result_operation: None,
        }
    }
}

pub(crate) fn send_log_content(
    communication: &InterAgentCommunication,
) -> Option<Cow<'static, str>> {
    tracing::enabled!(target: AGENT_COMMUNICATION_TARGET, tracing::Level::INFO).then(|| {
        match &communication.encrypted_content {
            Some(content) => Cow::Owned(content.clone()),
            None => Cow::Borrowed("[plaintext]"),
        }
    })
}

pub(crate) fn emit_agent_communication_send(
    communication_id: &str,
    context: &AgentCommunicationContext,
    log_content: Option<&str>,
    receiver_thread_id: ThreadId,
) {
    let Some(content) = log_content else {
        return;
    };
    tracing::info!(
        target: AGENT_COMMUNICATION_TARGET,
        {
            event.name = "codex.agent_communication",
            communication_id,
            kind = context.kind.as_str(),
            state = "send",
            sender_thread_id = %context.sender_thread_id,
            receiver_thread_id = %receiver_thread_id,
            content,
        },
        "agent communication"
    );
}

pub(crate) fn emit_agent_communication_receive(communication_id: &str) {
    tracing::info!(
        target: AGENT_COMMUNICATION_TARGET,
        {
            event.name = "codex.agent_communication",
            communication_id,
            state = "receive",
        },
        "agent communication"
    );
}

/// Pending mail with shared ownership of its captured publication state.
/// Serialization preserves the protocol shape; restored historical mail is not republished.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PendingAgentMessage {
    communication: InterAgentCommunication,
    #[serde(skip)]
    pub(crate) delivery: Option<MessageDelivery>,
}

impl From<InterAgentCommunication> for PendingAgentMessage {
    fn from(communication: InterAgentCommunication) -> Self {
        Self::new(communication, /*delivery*/ None)
    }
}

impl Deref for PendingAgentMessage {
    type Target = InterAgentCommunication;
    fn deref(&self) -> &Self::Target {
        &self.communication
    }
}

impl PartialEq for PendingAgentMessage {
    fn eq(&self, other: &Self) -> bool {
        self.communication == other.communication
    }
}

impl PendingAgentMessage {
    /// A terminal notification remains eligible only while native mail is unread.
    pub(crate) fn result_sender(&self) -> Option<ThreadId> {
        self.delivery
            .as_ref()
            .and_then(MessageDelivery::result_sender)
    }

    pub(crate) fn new(
        communication: InterAgentCommunication,
        delivery: Option<MessageDelivery>,
    ) -> Self {
        Self {
            communication,
            delivery,
        }
    }
}

/// The publication state retained until this delivery is consumed or abandoned.
struct MessagePublication {
    input: MessageLifecycleInput,
    observers: Box<[Arc<dyn MessageLifecycleContributor>]>,
    result_sender: Option<ThreadId>,
    final_capture: Option<Arc<FinalCaptureAttachment>>,
    result_operation: Option<String>,
}

/// Clones follow one logical delivery through queues and share its consumption state.
#[derive(Clone)]
pub(crate) struct MessageDelivery(Arc<Mutex<Option<MessagePublication>>>);

impl MessageDelivery {
    pub(crate) fn publish(
        sender: ThreadId,
        observers: &[Arc<dyn MessageLifecycleContributor>],
    ) -> Self {
        let input = MessageLifecycleInput {
            sender,
            data: ExtensionData::new(uuid::Uuid::now_v7().to_string()),
        };
        for observer in observers {
            observer.on_publish(&input);
        }
        let publication = MessagePublication {
            input,
            observers: observers.into(),
            result_sender: None,
            final_capture: None,
            result_operation: None,
        };
        Self(Arc::new(Mutex::new(Some(publication))))
    }

    pub(crate) fn publish_with_context(
        context: &AgentCommunicationContext,
        observers: &[Arc<dyn MessageLifecycleContributor>],
    ) -> Self {
        let delivery = Self::publish(context.sender_thread_id, observers);
        if context.kind == AgentCommunicationKind::Result {
            let mut publication = delivery.0.lock().unwrap_or_else(PoisonError::into_inner);
            let publication = publication.as_mut().expect("new publication");
            publication.result_sender = Some(context.sender_thread_id);
            publication.result_operation = context.result_operation.clone();
            publication.final_capture = context.final_capture.clone();
        }
        delivery
    }

    pub(crate) fn final_capture(&self) -> Option<Arc<FinalCaptureAttachment>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|publication| publication.final_capture.clone())
    }

    pub(crate) fn result_operation(&self) -> Option<(ThreadId, String)> {
        let publication = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let publication = publication.as_ref()?;
        Some((
            publication.result_sender?,
            publication.result_operation.clone()?,
        ))
    }

    fn result_sender(&self) -> Option<ThreadId> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|publication| publication.result_sender)
    }

    /// Commit this delivery and release its publication state, even if clones remain.
    pub(crate) fn consumed(&self, recipient: ThreadId) {
        let publication = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(publication) = publication {
            for observer in &publication.observers {
                observer.on_consumed(&publication.input, recipient);
            }
        }
    }
}

impl fmt::Debug for MessageDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageDelivery").finish_non_exhaustive()
    }
}
