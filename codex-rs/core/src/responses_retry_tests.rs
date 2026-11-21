use super::ResponsesStreamRequest;
use super::ResponsesStreamRetryState;
use super::handle_response_stream_error;
use super::log_retry;
use super::retry_wait_message;
use crate::realtime_history::RealtimeHistoryState;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
use crate::session::tests::make_session_and_context_with_auth_and_config_and_rx;
use codex_http_client::RetryAfter;
use codex_login::CodexAuth;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::EventMsg;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing_test::internal::MockWriter;

#[tokio::test]
async fn sampling_retry_logs_stream_error_context() {
    let (_session, turn_context) = make_session_and_context().await;
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    log_retry(
        ResponsesStreamRequest::Sampling,
        &turn_context,
        &CodexErr::Stream("websocket closed by server before response.completed".to_string()),
        /*retries*/ 2,
        /*max_retries*/ 5,
        Duration::from_secs(1),
    );

    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("retry log should be valid utf-8");
    assert!(logs.contains("stream disconnected - retrying sampling request"));
    assert!(logs.contains(&format!("turn_id={}", turn_context.sub_id)));
    assert!(logs.contains("retries=2"));
    assert!(logs.contains("max_retries=5"));
    assert!(logs.contains(
        "sampling_error=stream disconnected before completion: websocket closed by server before response.completed"
    ));
}

/// Time spent reporting a retry must count toward the server's original deadline.
#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "test holds the event-delivery lock to delay notification while virtual time advances"
)]
async fn stream_retry_preserves_deadline_across_delayed_notification() {
    let (mut session, turn_context) = make_session_and_context().await;
    let step_context = StepContext::for_test(Arc::new(turn_context));
    session.realtime_history = Some(Mutex::new(RealtimeHistoryState::default()));
    let mut client_session = session.services.model_client.new_session();
    // The second retry emits a notification even when release builds hide the first.
    let mut retry_state = ResponsesStreamRetryState {
        retries: 1,
        ..Default::default()
    };

    tokio::time::pause();
    let advice = RetryAfter::from_delay(Duration::from_secs(10)).expect("retry deadline");
    tokio::time::advance(Duration::from_secs(4)).await;

    // Hold the event-delivery lock so reporting the error consumes part of the deadline.
    let history_guard = session.realtime_history.as_ref().unwrap().lock().await;
    let retry = handle_response_stream_error(
        &mut retry_state,
        /*max_retries*/ 2,
        CodexErr::InternalServerError.with_retry_after(advice),
        &mut client_session,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    );
    tokio::pin!(retry);
    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(Duration::from_secs(4)).await;
    drop(history_guard);

    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(futures::poll!(&mut retry).is_pending());
    retry.await.expect("retry should be allowed");
    // Tokio rounds timer deadlines up to the next millisecond.
    let resumed_at = Instant::now();
    assert!(
        (advice.deadline()..=advice.deadline() + Duration::from_millis(1)).contains(&resumed_at),
        "retry resumed at {resumed_at:?}, expected {advice:?}"
    );
}

#[test_case::test_case(None, 1, "Retrying in 1s"; "without_advice")]
#[test_case::test_case(Some(311), 311, "Retrying in 5m 11s"; "with_advice")]
#[tokio::test]
async fn first_capacity_retry_reports_wait_before_http_fallback(
    advice_seconds: Option<u64>,
    wait_seconds: u64,
    message: &str,
) {
    let (session, turn_context, events) = make_session_and_context_with_auth_and_config_and_rx(
        CodexAuth::from_api_key("Test API Key"),
        Vec::new(),
        |config| config.model_provider.supports_websockets = true,
    )
    .await;
    let step_context = StepContext::for_test(turn_context);
    let mut client_session = session.services.model_client.new_session();
    let mut retry_state = ResponsesStreamRetryState::default();

    tokio::time::pause();
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    let mut error = CodexErr::new(CodexErrorDetails::ServerOverloaded);
    if let Some(seconds) = advice_seconds {
        error = error.with_retry_after(
            RetryAfter::from_delay(Duration::from_secs(seconds)).expect("retry deadline"),
        );
    }
    let retry = handle_response_stream_error(
        &mut retry_state,
        /*max_retries*/ 0,
        error,
        &mut client_session,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    );
    tokio::pin!(retry);
    assert!(futures::poll!(&mut retry).is_pending());
    let notice = std::iter::from_fn(|| events.try_recv().ok()).find_map(|event| match event.msg {
        EventMsg::StreamError(notice) => Some(notice),
        _ => None,
    });
    assert_eq!(notice.expect("visible fallback wait").message, message);
    tokio::time::advance(Duration::from_secs(wait_seconds - 1)).await;
    assert!(futures::poll!(&mut retry).is_pending());
    retry.await.expect("fallback should retry");
    assert!((deadline..=deadline + Duration::from_millis(1)).contains(&Instant::now()));
    assert!(!session.services.model_client.responses_websocket_enabled());
}

#[tokio::test]
async fn unadvised_capacity_backoff_doubles_to_thirty_seconds() {
    let (session, turn_context) = make_session_and_context().await;
    let step_context = StepContext::for_test(Arc::new(turn_context));
    let mut client_session = session.services.model_client.new_session();
    let mut retry_state = ResponsesStreamRetryState::default();

    tokio::time::pause();
    for seconds in [1, 2, 4, 8, 16, 30, 30] {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        handle_response_stream_error(
            &mut retry_state,
            /*max_retries*/ 7,
            CodexErr::new(CodexErrorDetails::ServerOverloaded),
            &mut client_session,
            &session,
            &step_context,
            ResponsesStreamRequest::Sampling,
        )
        .await
        .expect("capacity retry should be allowed");
        assert!((deadline..=deadline + Duration::from_millis(1)).contains(&Instant::now()));
    }
}

#[test_case::test_case(0, "Retrying now"; "zero")]
#[test_case::test_case(45, "Retrying in 45s"; "seconds")]
#[test_case::test_case(311, "Retrying in 5m 11s"; "minutes")]
#[test_case::test_case(3600, "Retrying in 1h 00m"; "one_hour")]
#[test_case::test_case(7500, "Retrying in 2h 05m"; "hours_and_minutes")]
#[test_case::test_case(259_745, "Retrying in 3d 0h 09m"; "multi_day_weekly_reset")]
#[tokio::test]
async fn retry_wait_message_formats_multi_hour_and_multi_day_waits(seconds: u64, expected: &str) {
    tokio::time::pause();
    let retry_at = Instant::now() + Duration::from_secs(seconds);
    assert_eq!(retry_wait_message(retry_at), expected);
}
