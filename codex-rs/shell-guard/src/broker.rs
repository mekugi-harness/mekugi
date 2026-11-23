use crate::protocol::MAX_MESSAGE_BYTES;
use crate::protocol::ShimReply;
use crate::protocol::ShimRequest;
use crate::protocol::TIMEOUT;
use crate::secret::SecretString;
use std::future::Future;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::sync::Semaphore;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const MAX_IN_FLIGHT_CONNECTIONS: usize = 64;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const REPLY_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Reason a pending shell-guard request resolved without an explicit user answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGuardResolvedReason {
    Withdrawn,
    TimedOut,
}

/// Decision delivered back to a waiting guard shim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellGuardBrokerDecision {
    Allow,
    ProvideSecret(SecretString),
    Deny(String),
}

impl ShellGuardBrokerDecision {
    fn into_reply(self) -> ShimReply {
        match self {
            Self::Allow => ShimReply::allow(),
            Self::ProvideSecret(secret) => ShimReply::provide_secret(secret),
            Self::Deny(reason) => ShimReply::deny(reason),
        }
    }
}

/// Runtime handler bridging local UDS shim requests to the session's event/request pipeline.
///
/// Implementations must register the pending request and emit the corresponding event before
/// returning the `(request_id, receiver)` pair. If `resolved` is invoked for a `request_id`,
/// the implementation should clean up any pending state and notify the client that the server
/// request is no longer active.
pub trait ShellGuardHandler: Send + Sync + 'static {
    fn handle_request(
        &self,
        request: ShimRequest,
    ) -> impl Future<Output = Option<(String, oneshot::Receiver<ShellGuardBrokerDecision>)>> + Send;

    fn handle_resolved(
        &self,
        request_id: String,
        reason: ShellGuardResolvedReason,
    ) -> impl Future<Output = ()> + Send;
}

/// Configuration for [`ShellGuardBroker`].
#[derive(Debug, Clone)]
pub struct ShellGuardBrokerConfig {
    pub timeout: Duration,
    pub max_in_flight: usize,
}

impl Default for ShellGuardBrokerConfig {
    fn default() -> Self {
        Self {
            timeout: TIMEOUT,
            max_in_flight: MAX_IN_FLIGHT_CONNECTIONS,
        }
    }
}

/// Session-private local Unix domain socket broker serving guard shims.
pub struct ShellGuardBroker {
    socket_path: PathBuf,
    cancel: CancellationToken,
    accept_task: Option<JoinHandle<()>>,
    _socket_dir: TempDir,
}

impl ShellGuardBroker {
    /// Starts a new broker listening on a private short-path Unix socket.
    #[tracing::instrument(level = "debug", skip(handler))]
    pub async fn start<H: ShellGuardHandler>(
        handler: Arc<H>,
        config: ShellGuardBrokerConfig,
    ) -> io::Result<Self> {
        let base_dir = short_socket_base_dir();
        let socket_dir = tempfile::Builder::new()
            .prefix("codex-sg-")
            .tempdir_in(&base_dir)?;
        codex_uds::prepare_private_socket_directory(socket_dir.path()).await?;

        let socket_path = socket_dir.path().join("s.sock");
        let listener = codex_uds::UnixListener::bind(&socket_path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        }

        let cancel = CancellationToken::new();
        let accept_cancel = cancel.clone();
        let semaphore = Arc::new(Semaphore::new(config.max_in_flight.max(1)));
        let timeout = config.timeout;

        let accept_task = tokio::spawn(async move {
            run_accept_loop(listener, handler, accept_cancel, semaphore, timeout).await;
        });

        Ok(Self {
            socket_path,
            cancel,
            accept_task: Some(accept_task),
            _socket_dir: socket_dir,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stops accepting new connections and cancels any in-flight waits.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.accept_task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ShellGuardBroker {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.accept_task.take() {
            task.abort();
        }
    }
}

fn short_socket_base_dir() -> PathBuf {
    #[cfg(unix)]
    {
        let tmp = Path::new("/tmp");
        if tmp.is_dir() {
            return tmp.to_path_buf();
        }
    }
    std::env::temp_dir()
}

async fn run_accept_loop<H: ShellGuardHandler>(
    mut listener: codex_uds::UnixListener,
    handler: Arc<H>,
    cancel: CancellationToken,
    semaphore: Arc<Semaphore>,
    timeout: Duration,
) {
    let mut workers = tokio::task::JoinSet::new();
    loop {
        let permit = tokio::select! {
            _ = cancel.cancelled() => break,
            permit = semaphore.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };

        let stream = tokio::select! {
            _ = cancel.cancelled() => {
                drop(permit);
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::debug!("shell guard socket accept error: {err}");
                    drop(permit);
                    continue;
                }
            },
        };

        let handler = Arc::clone(&handler);
        let conn_cancel = cancel.clone();
        workers.spawn(async move {
            let _permit = permit;
            handle_connection(stream, handler, conn_cancel, timeout).await;
        });

        while workers.try_join_next().is_some() {}
    }

    while workers.join_next().await.is_some() {}
}

#[tracing::instrument(level = "debug", skip_all)]
async fn handle_connection<H: ShellGuardHandler>(
    stream: codex_uds::UnixStream,
    handler: Arc<H>,
    cancel: CancellationToken,
    timeout: Duration,
) {
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);

    let request = match tokio::time::timeout(REQUEST_READ_TIMEOUT, read_request(&mut reader)).await
    {
        Ok(Ok(request)) => request,
        Ok(Err(reason)) => {
            let _ = write_reply(&mut write_half, &ShimReply::deny(reason)).await;
            return;
        }
        Err(_) => {
            let _ = write_reply(
                &mut write_half,
                &ShimReply::deny("timed out reading guard request"),
            )
            .await;
            return;
        }
    };

    if request.argv.is_empty() {
        let _ = write_reply(
            &mut write_half,
            &ShimReply::deny("invalid approval request"),
        )
        .await;
        return;
    }

    let Some((request_id, mut decision_rx)) = handler.handle_request(request).await else {
        let _ = write_reply(
            &mut write_half,
            &ShimReply::deny("approval module is not active"),
        )
        .await;
        return;
    };

    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);

    let reply = tokio::select! {
        decision = &mut decision_rx => {
            match decision {
                Ok(decision) => decision.into_reply(),
                Err(_) => ShimReply::deny("approval channel closed"),
            }
        }
        () = &mut sleep => {
            handler.handle_resolved(request_id, ShellGuardResolvedReason::TimedOut).await;
            ShimReply::deny("no answer within 5 minutes")
        }
        () = wait_for_peer_disconnect(&mut reader) => {
            handler.handle_resolved(request_id, ShellGuardResolvedReason::Withdrawn).await;
            return;
        }
        () = cancel.cancelled() => {
            handler.handle_resolved(request_id, ShellGuardResolvedReason::Withdrawn).await;
            ShimReply::deny("session closed")
        }
    };

    let _ = write_reply(&mut write_half, &reply).await;
}

async fn read_request<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<ShimRequest, String> {
    let mut line = Vec::new();
    let mut limited = reader.take((MAX_MESSAGE_BYTES + 1) as u64);
    let read = limited
        .read_until(b'\n', &mut line)
        .await
        .map_err(|_| "failed to read guard request".to_owned())?;
    if read == 0 {
        return Err("empty guard request".to_owned());
    }
    if line.len() > MAX_MESSAGE_BYTES {
        return Err("guard request exceeds size limit".to_owned());
    }
    serde_json::from_slice(&line).map_err(|_| "invalid guard request".to_owned())
}

async fn wait_for_peer_disconnect<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) {
    let mut buf = [0u8; 16];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

async fn write_reply<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    reply: &ShimReply,
) -> io::Result<()> {
    let mut encoded = zeroize::Zeroizing::new(
        serde_json::to_vec(reply).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?,
    );
    encoded.push(b'\n');
    let result = tokio::time::timeout(REPLY_WRITE_TIMEOUT, async {
        writer.write_all(&encoded).await?;
        writer.flush().await
    })
    .await;
    match result {
        Ok(io_result) => io_result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "reply write timed out",
        )),
    }
}
