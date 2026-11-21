//! Shared retry and transport fallback decisions for Responses requests.
//! Content-filter guidance is recorded for sampling requests before retry decisions.
//! Server advice controls timing without extending configured retry limits.

use std::future::pending;
use std::time::Duration;

use crate::client::ModelClientSession;
use crate::context::ContentFilterGuidance;
use crate::context::ContextualUserFragment;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use chrono::Utc;
use codex_client::RetryOperation;
use codex_features::Feature;
use codex_http_client::RetryAfter;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use tokio::time::Instant;
use tracing::warn;

const INITIAL_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_CAPACITY_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Request-local backoff for capacity failures without provider advice.
struct CapacityRetryBackoff(Duration);

impl Default for CapacityRetryBackoff {
    fn default() -> Self {
        Self(Duration::from_secs(1))
    }
}

impl CapacityRetryBackoff {
    fn retry_at(&mut self, error: &CodexErr) -> Option<Instant> {
        if !matches!(
            error.details(),
            CodexErrorDetails::ServerOverloaded | CodexErrorDetails::PolicyCapacityUnavailable
        ) {
            return None;
        }
        if let Some(advice) = error.retry_after() {
            return Some(advice.deadline());
        }
        // Source: modules/threadcontrol/control/waits.go:373:382@[71e3c624d] waitForProvider
        let retry_at = Instant::now() + self.0;
        self.0 = self.0.saturating_mul(2).min(MAX_CAPACITY_RETRY_DELAY);
        Some(retry_at)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResponsesStreamRequest {
    Sampling,
    RemoteCompactionV2,
}

/// A new generation cannot repair a terminal provider response or settle the
/// previous attempt's bill. Preserve these outcomes across transport retries,
/// same-turn steering, and compaction's fallback to another model.
pub(crate) fn is_terminal_sampling_outcome(error: &CodexErr) -> bool {
    matches!(
        error.details(),
        CodexErrorDetails::ResponseIncomplete(_)
            | CodexErrorDetails::UsageAccountingFailed(_)
            | CodexErrorDetails::SessionBudgetExceeded
            | CodexErrorDetails::InferenceHookDenied { .. }
    )
}

pub(crate) struct ResponsesStreamRetryState {
    retries: u64,
    capacity_backoff: CapacityRetryBackoff,
    connection_retries: u64,
    connection_retry_delay: Duration,
}

impl Default for ResponsesStreamRetryState {
    fn default() -> Self {
        Self {
            retries: 0,
            capacity_backoff: CapacityRetryBackoff::default(),
            connection_retries: 0,
            connection_retry_delay: INITIAL_CONNECTION_RETRY_DELAY,
        }
    }
}

/// Server retry advice retained after stream retries are exhausted. The turn ID
/// prevents a reused Guardian session from applying advice from an earlier review.
pub(crate) struct ExhaustedResponseRetry {
    pub(crate) turn_id: String,
    pub(crate) retry_at: Option<tokio::time::Instant>,
}

/// Returns `Ok(())` when the caller should retry the request loop, or the original error when
/// it is terminal or the retry budget is exhausted.
#[tracing::instrument(level = "trace", skip_all, fields(request = ?request))]
pub(crate) async fn handle_response_stream_error(
    retry_state: &mut ResponsesStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    sess: &Session,
    step_context: &StepContext,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    let turn_context = &step_context.turn;
    if matches!(request, ResponsesStreamRequest::Sampling)
        && let CodexErrorDetails::UsageLimitReached(limit) = err.details()
    {
        if let Some(rate_limits) = limit.rate_limits.clone() {
            sess.update_rate_limits(turn_context, *rate_limits).await;
        }
        // Approval reviews and internal requests are not user turns that can be paused.
        if turn_context.session_source.is_internal()
            || crate::guardian::is_basic_session_source(&turn_context.session_source)
        {
            return Err(err);
        }
        let retry_at = limit
            .resets_at
            .and_then(|reset| {
                RetryAfter::from_delay((reset - Utc::now()).to_std().unwrap_or_default())
            })
            .or_else(|| err.retry_after())
            .map(RetryAfter::deadline);
        let message = match retry_at {
            Some(retry_at) => format!("Usage limit reached. {}", retry_wait_message(retry_at)),
            None => "Usage limit reached. Paused until new input or interruption.".to_owned(),
        };
        sess.notify_stream_error(turn_context, message, err).await;
        match retry_at {
            Some(retry_at) => tokio::time::sleep_until(retry_at).await,
            None => pending().await,
        }
        return Ok(());
    }
    if matches!(request, ResponsesStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ContentFilter)
    {
        let model_info = &step_context.settings.model_info;
        let guidance = ContentFilterGuidance {
            text: codex_prompts::ResolvedModelMessages::from_model(model_info)
                .content_filter_guidance()
                .to_string(),
        };
        sess.record_conversation_items(
            turn_context,
            model_info,
            &[ContextualUserFragment::into(guidance)],
        )
        .await;
    }
    let operation = match request {
        ResponsesStreamRequest::Sampling => RetryOperation::Sampling,
        ResponsesStreamRequest::RemoteCompactionV2 => RetryOperation::RemoteCompactionV2,
    };
    let retry_count = retry_state.retries.saturating_add(1);
    let capacity_retry_at = retry_state.capacity_backoff.retry_at(&err);
    let Some(delay) = capacity_retry_at
        .map(|retry_at| retry_at.saturating_duration_since(Instant::now()))
        .or_else(|| err.retry_delay(retry_count))
    else {
        return Err(err);
    };
    let advised_retry_at = err.retry_after().map(RetryAfter::deadline);

    if turn_context
        .config
        .features
        .enabled(Feature::UnboundedConnectionRetries)
        && matches!(request, ResponsesStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ConnectionFailed(_))
        && !turn_context.session_source.is_internal()
        && !turn_context.provider.info().is_amazon_bedrock()
    {
        let retry_delay = retry_state.connection_retry_delay;
        warn!(
            turn_id = %turn_context.sub_id,
            error = %err,
            ?retry_delay,
            "stream connection failed; waiting to retry"
        );
        sess.notify_stream_error(turn_context, "Reconnecting... waiting for network", err)
            .await;
        retry_state.connection_retries = retry_state.connection_retries.saturating_add(1);
        codex_client::record_retry!(retry_state.connection_retries, retry_delay, operation);
        tokio::time::sleep(retry_delay).await;
        retry_state.connection_retry_delay = retry_delay
            .saturating_mul(2)
            .min(MAX_CONNECTION_RETRY_DELAY);
        return Ok(());
    }

    let requires_http_fallback = err.requires_http_fallback();
    let switched = (requires_http_fallback || retry_state.retries >= max_retries)
        && client_session.try_switch_fallback_transport(
            &turn_context.session_telemetry,
            turn_context.model_info(),
        );
    // Another request can already have switched this session to HTTP.
    if switched || requires_http_fallback {
        if switched {
            let message = format!("Falling back from WebSockets to HTTPS transport. {err:#}");
            if let Some(retry_at) = capacity_retry_at {
                sess.notify_stream_error(turn_context, retry_wait_message(retry_at), err)
                    .await;
            }
            // Changing transport must not bypass a capacity wait or provider advice.
            if let Some(retry_at) = capacity_retry_at.or(advised_retry_at) {
                tokio::time::sleep_until(retry_at).await;
            }
            sess.send_event(turn_context, EventMsg::Warning(WarningEvent { message }))
                .await;
        }
        retry_state.retries = 0;
        return Ok(());
    }

    if retry_state.retries < max_retries {
        retry_state.retries = retry_count;
        log_retry(request, turn_context, &err, retry_count, max_retries, delay);

        // Keep the first transient WebSocket reconnect quiet in release builds, but
        // show capacity waits from the first attempt.
        let report_error = capacity_retry_at.is_some()
            || retry_count > 1
            || cfg!(debug_assertions)
            || !sess.services.model_client.responses_websocket_enabled();
        if report_error {
            let message = match capacity_retry_at {
                Some(retry_at) => format!(
                    "{} ({retry_count}/{max_retries})",
                    retry_wait_message(retry_at)
                ),
                None => format!("Reconnecting... {retry_count}/{max_retries}"),
            };
            sess.notify_stream_error(turn_context, message, err).await;
        }
        // Use one clock sample so local backoff telemetry retains the selected delay.
        let now = Instant::now();
        let retry_at = capacity_retry_at
            .or(advised_retry_at)
            .unwrap_or(now + delay);
        let delay = retry_at.saturating_duration_since(now);
        codex_client::record_retry!(retry_count, delay, operation);
        tokio::time::sleep_until(retry_at).await;
        return Ok(());
    }

    sess.services
        .thread_extension_data
        .insert(ExhaustedResponseRetry {
            turn_id: turn_context.sub_id.clone(),
            retry_at: advised_retry_at,
        });
    Err(err)
}

fn retry_wait_message(retry_at: Instant) -> String {
    let delay = retry_at.saturating_duration_since(Instant::now());
    let seconds = delay
        .as_secs()
        .saturating_add(u64::from(delay.subsec_nanos() != 0));
    match seconds {
        0 => "Retrying now".to_string(),
        1..60 => format!("Retrying in {seconds}s"),
        60..3600 => format!("Retrying in {}m {:02}s", seconds / 60, seconds % 60),
        3600..86_400 => format!(
            "Retrying in {}h {:02}m",
            seconds / 3600,
            (seconds % 3600) / 60
        ),
        _ => format!(
            "Retrying in {}d {}h {:02}m",
            seconds / 86_400,
            (seconds % 86_400) / 3600,
            (seconds % 3600) / 60
        ),
    }
}

fn log_retry(
    request: ResponsesStreamRequest,
    turn_context: &TurnContext,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    match request {
        ResponsesStreamRequest::Sampling => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                sampling_error = %err,
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );
        }
        ResponsesStreamRequest::RemoteCompactionV2 => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                compact_error = %err,
                "remote compaction v2 stream failed; retrying request after delay"
            );
        }
    }
}

#[cfg(test)]
#[path = "responses_retry_tests.rs"]
mod tests;
