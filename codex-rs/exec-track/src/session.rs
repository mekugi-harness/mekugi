//! Native Unix control/ack exec-tracking session and report collector.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::os::unix::io::OwnedFd;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;
use tempfile::TempDir;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::rewrite::RewriteOptions;
use crate::rewrite::TrackingMode;
use crate::rewrite::instrument_script;
use crate::segment::SegmentSpan;
use crate::segment::ShellDialect;

/// Per-session retained segment output budget (4 MiB, matching SPEC.md).
const MAX_SEGMENT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// Output stream tag for bytes fed from the native output collector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStreamKind {
    Stdout,
    Stderr,
}

/// Request sent by the control loop to the native output collector so all
/// currently buffered stdout/stderr bytes are assigned to the current segment
/// before acknowledging a shell boundary.
pub struct DrainRequest {
    ack: oneshot::Sender<()>,
}

impl DrainRequest {
    pub fn complete(self) {
        let _ = self.ack.send(());
    }
}

/// Execution state of one tracked segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SegmentStatus {
    Pending,
    Running,
    Completed { exit_code: i32 },
    Skipped,
}

/// Per-segment execution record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentReport {
    pub index: usize,
    pub source: String,
    pub status: SegmentStatus,
    pub duration: Option<Duration>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub combined: Vec<u8>,
}

/// One actual successful mktemp creation observed during execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MktempCreation {
    pub path: PathBuf,
    pub is_dir: bool,
    pub segment_index: usize,
}

/// Final settled exec-tracking report for one command invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecTrackReport {
    pub segments: Vec<SegmentReport>,
    pub mktemp_creations: Vec<MktempCreation>,
    pub exit_code: Option<i32>,
    /// True only when the shell completed the tracking handshake and agreed
    /// with the host exit status without lossy truncation.
    pub tracked: bool,
    /// True if evidence was truncated or partially lost.
    pub incomplete_evidence: bool,
    pub fallback_reason: Option<String>,
}

struct SharedState {
    mode: TrackingMode,
    opened: bool,
    shell_exited: bool,
    tracked_exit_code: Option<i32>,
    active_segment: Option<usize>,
    last_segment: Option<usize>,
    started_at: Vec<Option<Instant>>,
    segments: Vec<SegmentReport>,
    total_output_bytes: usize,
    output_truncated: bool,
}

impl SharedState {
    fn new(spans: &[SegmentSpan], mode: TrackingMode) -> Self {
        let segments = spans
            .iter()
            .map(|span| SegmentReport {
                index: span.index,
                source: span.source.clone(),
                status: SegmentStatus::Pending,
                duration: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
                combined: Vec::new(),
            })
            .collect();
        Self {
            mode,
            opened: false,
            shell_exited: false,
            tracked_exit_code: None,
            active_segment: None,
            last_segment: None,
            started_at: vec![None; spans.len()],
            segments,
            total_output_bytes: 0,
            output_truncated: false,
        }
    }

    fn push_output(&mut self, stream: OutputStreamKind, bytes: &[u8]) {
        if bytes.is_empty() || self.mode == TrackingMode::Tty {
            return;
        }
        let target = self
            .active_segment
            .or(self.last_segment)
            .or_else(|| (!self.segments.is_empty()).then_some(0));
        let Some(idx) = target else {
            return;
        };
        let Some(seg) = self.segments.get_mut(idx) else {
            return;
        };
        if self.total_output_bytes.saturating_add(bytes.len()) > MAX_SEGMENT_OUTPUT_BYTES {
            self.output_truncated = true;
            return;
        }
        self.total_output_bytes += bytes.len();
        match stream {
            OutputStreamKind::Stdout => seg.stdout.extend_from_slice(bytes),
            OutputStreamKind::Stderr => seg.stderr.extend_from_slice(bytes),
        }
        seg.combined.extend_from_slice(bytes);
    }

    fn on_open(&mut self) {
        self.opened = true;
    }

    fn on_begin(&mut self, index: usize) {
        if index >= self.segments.len() {
            return;
        }
        self.opened = true;
        self.active_segment = Some(index);
        self.last_segment = Some(index);
        self.started_at[index] = Some(Instant::now());
        self.segments[index].status = SegmentStatus::Running;
    }

    fn on_end(&mut self, index: usize, exit_code: i32) {
        if index >= self.segments.len() {
            return;
        }
        let elapsed = self.started_at[index]
            .map(|start| start.elapsed())
            .unwrap_or(Duration::ZERO);
        self.segments[index].status = SegmentStatus::Completed { exit_code };
        self.segments[index].duration = Some(elapsed);
        if self.active_segment == Some(index) {
            self.active_segment = None;
        }
        self.last_segment = Some(index);
    }

    fn on_done(&mut self, exit_code: i32) {
        self.opened = true;
        self.shell_exited = true;
        self.tracked_exit_code = Some(exit_code);
        if let Some(active) = self.active_segment.take() {
            let elapsed = self.started_at[active]
                .map(|start| start.elapsed())
                .unwrap_or(Duration::ZERO);
            self.segments[active].status = SegmentStatus::Completed { exit_code };
            self.segments[active].duration = Some(elapsed);
            self.last_segment = Some(active);
        }
        for seg in &mut self.segments {
            if seg.status == SegmentStatus::Pending {
                seg.status = SegmentStatus::Skipped;
            }
        }
    }
}

/// Active native exec-tracking session for a single shell invocation.
pub struct ExecTrackSession {
    dir: TempDir,
    ctl_path: PathBuf,
    ack_path: PathBuf,
    mktemp_path: PathBuf,
    state: Arc<Mutex<SharedState>>,
    ack_fd: Mutex<Option<Arc<OwnedFd>>>,
    ctl_task: Mutex<Option<JoinHandle<()>>>,
    settled: OnceLock<ExecTrackReport>,
}

impl std::fmt::Debug for ExecTrackSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecTrackSession")
            .field("dir", &self.dir.path())
            .field("settled", &self.settled.get().is_some())
            .finish_non_exhaustive()
    }
}

impl ExecTrackSession {
    /// Start a new private tracking session for segments in mode.
    pub fn start(
        segments: Vec<SegmentSpan>,
        mode: TrackingMode,
    ) -> io::Result<(Self, mpsc::UnboundedReceiver<DrainRequest>)> {
        let dir = tempfile::Builder::new()
            .prefix("mekugi-exec-track-")
            .tempdir()?;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;

        let ctl_path = dir.path().join("ctl");
        let ack_path = dir.path().join("ack");
        let mktemp_path = dir.path().join("mktemp.log");

        mkfifo(&ctl_path, 0o600)?;
        mkfifo(&ack_path, 0o600)?;
        fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&mktemp_path)?;
        fs::set_permissions(&mktemp_path, fs::Permissions::from_mode(0o600))?;

        let ctl_fd = open_fifo_rdwr_nonblock(&ctl_path)?;
        let ack_fd = Arc::new(open_fifo_rdwr_nonblock(&ack_path)?);

        let state = Arc::new(Mutex::new(SharedState::new(&segments, mode)));
        let (drain_tx, drain_rx) = mpsc::unbounded_channel();

        let ctl_state = Arc::clone(&state);
        let ctl_ack_fd = Arc::clone(&ack_fd);
        let ctl_task = tokio::spawn(run_control_loop(ctl_fd, ctl_ack_fd, ctl_state, drain_tx));

        Ok((
            Self {
                dir,
                ctl_path,
                ack_path,
                mktemp_path,
                state,
                ack_fd: Mutex::new(Some(ack_fd)),
                ctl_task: Mutex::new(Some(ctl_task)),
                settled: OnceLock::new(),
            },
            drain_rx,
        ))
    }

    pub fn dir_path(&self) -> &Path {
        self.dir.path()
    }

    pub fn ctl_path(&self) -> &Path {
        &self.ctl_path
    }

    pub fn ack_path(&self) -> &Path {
        &self.ack_path
    }

    pub fn mktemp_path(&self) -> &Path {
        &self.mktemp_path
    }

    /// Build the instrumented single-shell script bound to this session's paths.
    pub fn instrument_script(
        &self,
        script: &str,
        segments: &[SegmentSpan],
        dialect: ShellDialect,
    ) -> String {
        let mode = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .mode;
        instrument_script(
            script,
            segments,
            &RewriteOptions {
                dialect,
                mode,
                ctl_path: self.ctl_path.clone(),
                ack_path: self.ack_path.clone(),
                mktemp_path: self.mktemp_path.clone(),
            },
        )
    }

    /// Record output bytes read from the native stdout/stderr collector into
    /// the active (or last) segment.
    pub fn push_output(&self, stream: OutputStreamKind, bytes: &[u8]) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_output(stream, bytes);
    }

    /// Abort the control task and unlink control/ack FIFOs so a running shell
    /// never blocks if observation is stopped or shut down early.
    pub fn stop(&self) {
        if let Some(handle) = self
            .ctl_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            handle.abort();
        }
        self.ack_fd
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let _ = fs::remove_file(&self.ctl_path);
        let _ = fs::remove_file(&self.ack_path);
    }

    /// Settle the session once (idempotent) after output has drained/closed.
    pub fn finalize(&self, host_exit_code: Option<i32>) -> ExecTrackReport {
        self.settled
            .get_or_init(|| {
                self.stop();
                let mktemp_creations = parse_mktemp_log(&self.mktemp_path);
                let guard = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);

                let mut fallback_reason = None;
                let mut tracked = true;
                if !guard.opened {
                    tracked = false;
                    fallback_reason = Some("tracker handshake was not received".to_string());
                } else if !guard.shell_exited {
                    tracked = false;
                    fallback_reason = Some("shell exit boundary was not received".to_string());
                } else if guard.output_truncated {
                    tracked = false;
                    fallback_reason =
                        Some("segment output exceeded 4 MiB retention cap".to_string());
                } else if let (Some(host), Some(reported)) =
                    (host_exit_code, guard.tracked_exit_code)
                    && host != reported
                {
                    tracked = false;
                    fallback_reason = Some(format!(
                        "tracked exit code ({reported}) disagreed with host exit code ({host})"
                    ));
                }

                let incomplete_evidence = !tracked || guard.output_truncated;

                ExecTrackReport {
                    segments: guard.segments.clone(),
                    mktemp_creations,
                    exit_code: host_exit_code.or(guard.tracked_exit_code),
                    tracked,
                    incomplete_evidence,
                    fallback_reason,
                }
            })
            .clone()
    }
}

impl Drop for ExecTrackSession {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_control_loop(
    ctl_fd: OwnedFd,
    ack_fd: Arc<OwnedFd>,
    state: Arc<Mutex<SharedState>>,
    drain_tx: mpsc::UnboundedSender<DrainRequest>,
) {
    let Ok(async_ctl) = AsyncFd::new(ctl_fd) else {
        return;
    };
    let mut pending = Vec::<u8>::new();
    let mut buf = [0u8; 4096];

    loop {
        let Ok(mut guard) = async_ctl.readable().await else {
            break;
        };
        let mut read_any = false;
        loop {
            let n = unsafe {
                libc::read(
                    async_ctl.as_raw_fd(),
                    buf.as_mut_ptr().cast::<libc::c_void>(),
                    buf.len(),
                )
            };
            if n > 0 {
                read_any = true;
                pending.extend_from_slice(&buf[..n as usize]);
            } else if n == 0 {
                break;
            } else {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    break;
                }
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
        }
        if !read_any {
            guard.clear_ready();
            continue;
        }

        let mut should_exit = false;
        while let Some(nl) = pending.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = pending.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut parts = trimmed.split_whitespace();
            let op = parts.next().unwrap_or("");
            match op {
                "o" => {
                    state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .on_open();
                    write_ack(&ack_fd);
                }
                "b" => {
                    if let Some(idx) = parts.next().and_then(|s| s.parse::<usize>().ok()) {
                        request_output_drain(&drain_tx).await;
                        state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .on_begin(idx);
                    }
                    write_ack(&ack_fd);
                }
                "e" => {
                    if let (Some(idx), Some(status)) = (
                        parts.next().and_then(|s| s.parse::<usize>().ok()),
                        parts.next().and_then(|s| s.parse::<i32>().ok()),
                    ) {
                        request_output_drain(&drain_tx).await;
                        state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .on_end(idx, status);
                    }
                    write_ack(&ack_fd);
                }
                "d" => {
                    let status = parts
                        .next()
                        .and_then(|s| s.parse::<i32>().ok())
                        .unwrap_or(0);
                    request_output_drain(&drain_tx).await;
                    state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .on_done(status);
                    write_ack(&ack_fd);
                    should_exit = true;
                    break;
                }
                _ => {
                    write_ack(&ack_fd);
                }
            }
        }

        if should_exit {
            break;
        }
    }
}

async fn request_output_drain(drain_tx: &mpsc::UnboundedSender<DrainRequest>) {
    let (tx, rx) = oneshot::channel();
    if drain_tx.send(DrainRequest { ack: tx }).is_ok() {
        let _ = tokio::time::timeout(Duration::from_millis(50), rx).await;
    }
}

fn write_ack(ack_fd: &OwnedFd) {
    let payload = b"k\n";
    let _ = unsafe {
        libc::write(
            ack_fd.as_raw_fd(),
            payload.as_ptr().cast::<libc::c_void>(),
            payload.len(),
        )
    };
}

fn mkfifo(path: &Path, mode: libc::mode_t) -> io::Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), mode) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_fifo_rdwr_nonblock(path: &Path) -> io::Result<OwnedFd> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(crate) fn parse_mktemp_log(path: &Path) -> Vec<MktempCreation> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for record in bytes.split(|&b| b == 0) {
        if record.is_empty() {
            continue;
        }
        if !record.starts_with(b"m ") {
            continue;
        }
        let rest = &record[2..];
        let Some(sp1) = rest.iter().position(|&b| b == b' ') else {
            continue;
        };
        let Ok(seg_str) = std::str::from_utf8(&rest[..sp1]) else {
            continue;
        };
        let Ok(segment_index) = seg_str.parse::<usize>() else {
            continue;
        };
        let rest = &rest[sp1 + 1..];
        let Some(sp2) = rest.iter().position(|&b| b == b' ') else {
            continue;
        };
        let is_dir = match &rest[..sp2] {
            b"d" => true,
            b"f" => false,
            _ => continue,
        };
        let raw_path = &rest[sp2 + 1..];
        if raw_path.is_empty() {
            continue;
        }
        let path_buf = PathBuf::from(std::ffi::OsStr::from_bytes(raw_path));
        if !out
            .iter()
            .any(|existing: &MktempCreation| existing.path == path_buf)
        {
            out.push(MktempCreation {
                path: path_buf,
                is_dir,
                segment_index,
            });
        }
    }
    out
}
