//! Shared, non-consuming input-activity waits for native tools and hosts.

use crate::session::InputQueueActivity;
use crate::session::session::Session;
use codex_protocol::ThreadId;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::watch;

/// Non-consuming view of native input. Final mail is not ordinary addressed mail.
#[derive(Default)]
pub(crate) struct PendingWaitActivity {
    pub(crate) steered: bool,
    pub(crate) message: bool,
    pub(crate) results: HashSet<ThreadId>,
}

impl PendingWaitActivity {
    pub(crate) fn observe(&mut self, message: &crate::PendingAgentMessage) {
        if let Some(sender) = message.result_sender() {
            self.results.insert(sender);
        } else {
            self.message = true;
        }
    }
}

pub(crate) async fn pending_wait_activity(session: &Session, turn_id: &str) -> PendingWaitActivity {
    let turn_state = session
        .input_queue
        .turn_state_for_sub_id(&session.active_turn, turn_id)
        .await;
    session
        .input_queue
        .pending_wait_activity(turn_state.as_deref())
        .await
}

/// Why a native or host input-activity wait finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputWaitOutcome {
    /// Agent mail was observed, including a typed agent message from a notifying injection.
    MailboxActivity,
    /// User steering or another kind of notifying input was observed.
    Steered,
    /// The deadline elapsed or the input-activity source closed.
    TimedOut,
}

pub(crate) async fn wait_for_input_activity(
    session: &Session,
    turn_id: &str,
    timeout: Duration,
) -> InputWaitOutcome {
    tokio::time::timeout(timeout, async {
        let (activity, pending) = subscribe_input_activity(session, turn_id).await;
        wait_for_activity(activity, pending).await
    })
    .await
    .unwrap_or(InputWaitOutcome::TimedOut)
}

pub(crate) async fn subscribe_input_activity(
    session: &Session,
    turn_id: &str,
) -> (
    watch::Receiver<InputQueueActivity>,
    Option<InputQueueActivity>,
) {
    let turn_state = session
        .input_queue
        .turn_state_for_sub_id(&session.active_turn, turn_id)
        .await;
    session
        .input_queue
        .subscribe_activity(turn_state.as_deref())
        .await
}

pub(crate) async fn wait_for_activity(
    mut activity: watch::Receiver<InputQueueActivity>,
    pending: Option<InputQueueActivity>,
) -> InputWaitOutcome {
    let activity = match pending {
        Some(activity) => activity,
        None => match activity.changed().await {
            Ok(()) => *activity.borrow_and_update(),
            Err(_) => return InputWaitOutcome::TimedOut,
        },
    };
    match activity {
        InputQueueActivity::Mailbox => InputWaitOutcome::MailboxActivity,
        InputQueueActivity::Steer => InputWaitOutcome::Steered,
    }
}
