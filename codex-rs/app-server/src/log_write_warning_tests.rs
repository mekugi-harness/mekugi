use super::*;
use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessage;
use codex_analytics::AnalyticsEventsClient;
use codex_core::config::ConfigBuilder;
use codex_state::LogWriteFailureReporter;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

#[tokio::test]
async fn log_write_warning_is_broadcast_once() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(/*buffer*/ 4);
    let outgoing = Arc::new(OutgoingMessageSender::new(
        tx,
        AnalyticsEventsClient::disabled(),
    ));
    let reporter = LogWriteWarningReporter::new(&outgoing);
    reporter.report_failure("SQLite flush failed");
    let OutgoingEnvelope::Broadcast {
        message: OutgoingMessage::AppServerNotification(envelope),
    } = rx.try_recv()?
    else {
        panic!("expected a broadcast notification");
    };
    let ServerNotification::Warning(notification) = envelope.notification else {
        panic!("expected a warning");
    };
    assert_eq!(notification, WarningNotification {
        thread_id: None,
        message: "Mekugi couldn't save diagnostic logs to its local database. Check the server logs for details.".to_string(),
    });
    reporter.report_failure("another SQLite flush failed");
    assert!(rx.try_recv().is_err());
    Ok(())
}

struct IgnoreFailure;
impl LogWriteFailureReporter for IgnoreFailure {
    fn report_failure(&self, _diagnostic: &str) {}
}

#[tokio::test]
async fn log_write_warning_reports_failed_sqlite_flush() -> anyhow::Result<()> {
    use tracing_subscriber::layer::SubscriberExt;

    for attach_after_failure in [false, true] {
        let home = tempfile::tempdir()?;
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .build()
            .await?;
        let runtime = codex_state::StateRuntime::init(
            config.sqlite_config().clone(),
            "test-provider".to_string(),
        )
        .await?;
        let (tx, mut rx) = mpsc::channel(/*buffer*/ 4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            tx,
            AnalyticsEventsClient::disabled(),
        ));
        let reporter = LogWriteWarningReporter::new(&outgoing);
        let initial_reporter: Arc<dyn LogWriteFailureReporter> = if attach_after_failure {
            Arc::new(IgnoreFailure)
        } else {
            reporter.clone()
        };
        let layer = codex_state::log_db::start(runtime.clone(), initial_reporter);
        runtime.close().await;
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(layer.clone()),
            || {
                tracing::info!("first failed log write");
            },
        );
        layer.flush().await;
        assert!(layer.has_write_failure());
        if attach_after_failure {
            layer.set_failure_reporter(reporter.clone());
            if layer.has_write_failure() {
                reporter.notify_failure();
            }
        }
        assert!(matches!(
            rx.try_recv(),
            Ok(OutgoingEnvelope::Broadcast {
                message: OutgoingMessage::AppServerNotification(_),
                ..
            })
        ));
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(layer.clone()),
            || {
                tracing::info!("second failed log write");
            },
        );
        layer.flush().await;
        assert!(rx.try_recv().is_err());
    }
    Ok(())
}
