use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::WarningNotification;
use codex_state::LogWriteFailureReporter;

use crate::outgoing_message::NotificationTarget;
use crate::outgoing_message::OutgoingMessageSender;

const LOG_WRITE_WARNING: &str = "Mekugi couldn't save diagnostic logs to its local database. Check the server logs for details.";

pub(crate) struct LogWriteWarningReporter {
    outgoing: Weak<OutgoingMessageSender>,
    /// Permanently set by the first failure; limits warning delivery to one attempt
    /// for this reporter, even if later SQLite writes succeed.
    has_failure: AtomicBool,
}

impl LogWriteWarningReporter {
    pub(crate) fn new(outgoing: &Arc<OutgoingMessageSender>) -> Arc<Self> {
        Arc::new(Self {
            outgoing: Arc::downgrade(outgoing),
            has_failure: AtomicBool::new(false),
        })
    }

    pub(crate) fn notify_failure(&self) {
        if !self.has_failure.swap(true, Ordering::Relaxed)
            && let Some(outgoing) = self.outgoing.upgrade()
        {
            outgoing.try_send_server_notification(
                NotificationTarget::Broadcast,
                ServerNotification::Warning(WarningNotification {
                    thread_id: None,
                    message: LOG_WRITE_WARNING.to_string(),
                }),
            );
        }
    }
}

impl LogWriteFailureReporter for LogWriteWarningReporter {
    fn report_failure(&self, diagnostic: &str) {
        use std::io::Write;

        // Re-entering tracing would send this failure back to the failed SQLite sink.
        let _ = writeln!(std::io::stderr().lock(), "{diagnostic}");
        self.notify_failure();
    }
}

#[cfg(test)]
#[path = "log_write_warning_tests.rs"]
mod tests;
