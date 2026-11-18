use std::collections::HashMap;
use std::fmt::Display;
use std::fmt::Formatter;
use std::future::Future;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use super::SessionInner;

const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ERROR_BYTES: usize = 512;

#[derive(Default)]
pub(super) struct Observations(Mutex<HashMap<String, watch::Sender<Option<Liveness>>>>);

#[derive(Clone, Copy)]
struct Liveness {
    at: Instant,
    paused: bool,
}

pub(super) struct Observation<'a> {
    observations: &'a Observations,
    id: String,
    updates: watch::Receiver<Option<Liveness>>,
}

impl Observations {
    pub(super) fn register(&self, id: String) -> Observation<'_> {
        let (tx, updates) = watch::channel(None);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), tx);
        Observation {
            observations: self,
            id,
            updates,
        }
    }

    pub(super) fn report(&self, id: &str, paused: bool) {
        if let Some(sender) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            sender.send_replace(Some(Liveness {
                at: Instant::now(),
                paused,
            }));
        }
    }
}

impl Drop for Observation<'_> {
    fn drop(&mut self) {
        self.observations
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// Terminal failure while establishing a new gRPC code-mode session.
#[derive(Debug, PartialEq, Eq)]
pub enum GrpcSessionOpenError {
    /// Final status from OpenSession or the first read of its lease, with the bounded diagnostic.
    AdmissionStatus { code: tonic::Code, message: String },
    /// The client deadline elapsed while waiting for admission, including retries.
    AdmissionTimeout(String),
    /// Setup failed outside admission, for example during transport setup or tool subscription.
    Other(String),
}

impl Display for GrpcSessionOpenError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AdmissionStatus { message, .. }
            | Self::AdmissionTimeout(message)
            | Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for GrpcSessionOpenError {}

impl<T: Into<String>> From<T> for GrpcSessionOpenError {
    fn from(message: T) -> Self {
        Self::Other(message.into())
    }
}

pub(super) async fn startup<T, E: Display>(
    operation: &str,
    request: impl Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match enforce(operation, Duration::ZERO, request).await {
        Ok(result) => Ok(result),
        Err(RequestError::Failed(error)) => Err(failure(operation, error)),
        Err(RequestError::TimedOut(reason)) => Err(reason),
    }
}

pub(super) async fn request<T>(
    session: &SessionInner,
    operation: &str,
    runtime_timeout: Duration,
    request: impl Future<Output = Result<T, tonic::Status>>,
) -> Result<T, String> {
    finish_request(
        session,
        operation,
        enforce(operation, runtime_timeout, request),
    )
    .await
}

async fn finish_request<T>(
    session: &SessionInner,
    operation: &str,
    request: impl Future<Output = Result<T, RequestError<tonic::Status>>>,
) -> Result<T, String> {
    let result = tokio::select! {
        biased;
        _ = session.stopped.cancelled() => {
            return Err("gRPC code-mode session closed".to_string());
        }
        result = request => result,
    };

    match result {
        Ok(value) => Ok(value),
        Err(RequestError::Failed(error)) => {
            let reason = failure(operation, &error);
            // Only the proxy can declare a pinned route lost. Ordinary RPC and
            // connection failures must not discard a potentially live session.
            if session.route.is_some()
                && error.code() == tonic::Code::Unavailable
                && error
                    .metadata()
                    .get("x-code-mode-route-lost")
                    .is_some_and(|value| value == "true")
            {
                session.fail(reason.clone());
            }
            Err(reason)
        }
        Err(RequestError::TimedOut(reason)) => {
            session.fail(reason.clone());
            Err(reason)
        }
    }
}

/// Invalidates the session on RPCs where `NotFound` means the session is missing.
pub(super) async fn session_request<T>(
    session: &SessionInner,
    operation: &str,
    runtime_timeout: Duration,
    rpc: impl Future<Output = Result<T, tonic::Status>>,
) -> Result<T, String> {
    request(
        session,
        operation,
        runtime_timeout,
        session_rpc(session, operation, rpc),
    )
    .await
}

pub(super) async fn observation_request<T>(
    session: &SessionInner,
    operation: &str,
    runtime_timeout: Duration,
    observation: Observation<'_>,
    rpc: impl Future<Output = Result<T, tonic::Status>>,
) -> Result<T, String> {
    finish_request(
        session,
        operation,
        enforce_observation(
            operation,
            runtime_timeout,
            observation,
            session_rpc(session, operation, rpc),
        ),
    )
    .await
}

async fn session_rpc<T>(
    session: &SessionInner,
    operation: &str,
    rpc: impl Future<Output = Result<T, tonic::Status>>,
) -> Result<T, tonic::Status> {
    let result = rpc.await;
    if let Err(error) = &result
        && error.code() == tonic::Code::NotFound
    {
        session.fail(failure(operation, error));
    }
    result
}

async fn enforce_observation<T, E>(
    operation: &str,
    runtime_timeout: Duration,
    mut observation: Observation<'_>,
    rpc: impl Future<Output = Result<T, E>>,
) -> Result<T, RequestError<E>> {
    let ordinary_timeout = runtime_timeout.saturating_add(TRANSPORT_TIMEOUT);
    let timer = tokio::time::sleep(ordinary_timeout);
    tokio::pin!(rpc, timer);
    loop {
        // A pause is not an outcome. Refresh only on proof from the actor for
        // this observation, and restore its ordinary timer on resumption.
        if let Some(update) = *observation.updates.borrow_and_update() {
            let next = if update.paused {
                TRANSPORT_TIMEOUT
            } else {
                ordinary_timeout
            };
            timer.as_mut().reset(update.at + next);
        }
        tokio::select! {
            biased;
            result = &mut rpc => return result.map_err(RequestError::Failed),
            _ = &mut timer => return Err(timed_out(operation)),
            _ = observation.updates.changed() => {}
        }
    }
}

pub(super) fn failure(operation: &str, error: impl Display) -> String {
    let mut message = format!("gRPC code-mode {operation} failed: {error}");
    if message.len() > MAX_ERROR_BYTES {
        let boundary = message.floor_char_boundary(MAX_ERROR_BYTES - "...".len());
        message.truncate(boundary);
        message.push_str("...");
    }
    message
}

pub(super) async fn enforce<T, E>(
    operation: &str,
    runtime_timeout: Duration,
    request: impl Future<Output = Result<T, E>>,
) -> Result<T, RequestError<E>> {
    let timeout = runtime_timeout.saturating_add(TRANSPORT_TIMEOUT);
    match tokio::time::timeout(timeout, request).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(RequestError::Failed(error)),
        Err(_) => Err(timed_out(operation)),
    }
}

fn timed_out<E>(operation: &str) -> RequestError<E> {
    RequestError::TimedOut(format!(
        "gRPC code-mode host timed out waiting for {operation} response"
    ))
}

#[cfg_attr(test, derive(Debug))]
pub(super) enum RequestError<E> {
    Failed(E),
    TimedOut(String),
}

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod tests;
