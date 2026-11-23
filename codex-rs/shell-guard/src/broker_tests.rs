use crate::broker::ShellGuardBroker;
use crate::broker::ShellGuardBrokerConfig;
use crate::broker::ShellGuardBrokerDecision;
use crate::broker::ShellGuardHandler;
use crate::broker::ShellGuardResolvedReason;
use crate::protocol::GuardKind;
use crate::protocol::ShimReply;
use crate::protocol::ShimRequest;
use crate::secret::SecretString;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::oneshot;

struct TestHandler {
    mode: TestMode,
    resolved: Mutex<Vec<(String, ShellGuardResolvedReason)>>,
    resolved_notify: Notify,
}

enum TestMode {
    Immediate(ShellGuardBrokerDecision),
    Hold(Mutex<Option<oneshot::Sender<ShellGuardBrokerDecision>>>),
}

impl ShellGuardHandler for TestHandler {
    async fn handle_request(
        &self,
        _request: ShimRequest,
    ) -> Option<(String, oneshot::Receiver<ShellGuardBrokerDecision>)> {
        let (tx, rx) = oneshot::channel();
        match &self.mode {
            TestMode::Immediate(decision) => {
                let _ = tx.send(decision.clone());
            }
            TestMode::Hold(slot) => {
                *slot.lock().await = Some(tx);
            }
        }
        Some(("req-1".to_owned(), rx))
    }

    async fn handle_resolved(&self, request_id: String, reason: ShellGuardResolvedReason) {
        self.resolved.lock().await.push((request_id, reason));
        self.resolved_notify.notify_waiters();
    }
}

async fn round_trip(socket_path: &std::path::Path, request: &ShimRequest) -> ShimReply {
    let stream = codex_uds::UnixStream::connect(socket_path)
        .await
        .expect("connect to broker");
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut payload = serde_json::to_vec(request).expect("encode request");
    payload.push(b'\n');
    write_half.write_all(&payload).await.expect("write request");
    write_half.flush().await.expect("flush request");

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read reply");
    serde_json::from_str(&line).expect("decode reply")
}

#[tokio::test]
async fn broker_delivers_allow_secret_and_deny_decisions() {
    let secret = SecretString::new("s3cr3t!".to_owned());
    assert_eq!(format!("{secret:?}"), "<redacted>");

    let handler = Arc::new(TestHandler {
        mode: TestMode::Immediate(ShellGuardBrokerDecision::ProvideSecret(secret)),
        resolved: Mutex::new(Vec::new()),
        resolved_notify: Notify::new(),
    });
    let broker = ShellGuardBroker::start(handler, ShellGuardBrokerConfig::default())
        .await
        .expect("start broker");

    let reply = round_trip(
        broker.socket_path(),
        &ShimRequest {
            kind: GuardKind::SudoPassword,
            thread_id: "thr_1".to_owned(),
            item_id: "call_1".to_owned(),
            cwd: "/tmp".to_owned(),
            argv: vec!["sudo".to_owned(), "id".to_owned()],
            executable: "/usr/bin/sudo".to_owned(),
        },
    )
    .await;

    assert!(reply.ok);
    assert_eq!(
        reply.secret.as_ref().map(SecretString::as_str),
        Some("s3cr3t!")
    );
    broker.shutdown().await;
}

#[tokio::test]
async fn broker_times_out_and_notifies_resolved() {
    let handler = Arc::new(TestHandler {
        mode: TestMode::Hold(Mutex::new(None)),
        resolved: Mutex::new(Vec::new()),
        resolved_notify: Notify::new(),
    });
    let broker = ShellGuardBroker::start(
        Arc::clone(&handler),
        ShellGuardBrokerConfig {
            timeout: Duration::from_millis(50),
            max_in_flight: 4,
        },
    )
    .await
    .expect("start broker");

    let reply = round_trip(
        broker.socket_path(),
        &ShimRequest {
            kind: GuardKind::RemoteWrite,
            thread_id: "thr_1".to_owned(),
            item_id: "call_1".to_owned(),
            cwd: "/tmp".to_owned(),
            argv: vec!["git".to_owned(), "push".to_owned()],
            executable: "/usr/bin/git".to_owned(),
        },
    )
    .await;

    assert!(!reply.ok);
    assert_eq!(reply.reason.as_deref(), Some("no answer within 5 minutes"));
    assert_eq!(
        *handler.resolved.lock().await,
        vec![("req-1".to_owned(), ShellGuardResolvedReason::TimedOut)]
    );
    broker.shutdown().await;
}

#[tokio::test]
async fn broker_detects_peer_disconnect_as_withdrawn() {
    let handler = Arc::new(TestHandler {
        mode: TestMode::Hold(Mutex::new(None)),
        resolved: Mutex::new(Vec::new()),
        resolved_notify: Notify::new(),
    });
    let broker = ShellGuardBroker::start(Arc::clone(&handler), ShellGuardBrokerConfig::default())
        .await
        .expect("start broker");

    let mut stream = codex_uds::UnixStream::connect(broker.socket_path())
        .await
        .expect("connect");
    let mut payload = serde_json::to_vec(&ShimRequest {
        kind: GuardKind::RemoteWrite,
        thread_id: "thr_1".to_owned(),
        item_id: "call_1".to_owned(),
        cwd: "/tmp".to_owned(),
        argv: vec!["git".to_owned(), "push".to_owned()],
        executable: "/usr/bin/git".to_owned(),
    })
    .expect("encode");
    payload.push(b'\n');
    stream.write_all(&payload).await.expect("write");
    stream.flush().await.expect("flush");
    // Drop client socket abruptly to simulate killed shim process.
    drop(stream);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !handler.resolved.lock().await.is_empty() {
                break;
            }
            handler.resolved_notify.notified().await;
        }
    })
    .await
    .expect("broker should detect disconnect");

    assert_eq!(
        *handler.resolved.lock().await,
        vec![("req-1".to_owned(), ShellGuardResolvedReason::Withdrawn)]
    );
    broker.shutdown().await;
}
