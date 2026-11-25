use std::collections::VecDeque;
use std::io::Read;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;

use crate::ToolOutput;
use crate::common::to_valid_utf8_collapsed;

const MAX_OUTPUT_TOKENS: usize = 15_500;
const MAX_TOKENS_ARGUMENT_ERROR: &str =
    "--max-tokens requires one positive integer and cannot repeat";
const RUN_USAGE_ERROR: &str =
    "expected -n N or --max-tokens N, optionally --head or --tail, then [--] COMMAND [ARG...]";

#[derive(Clone, Debug, Eq, PartialEq)]
struct RunOptions {
    lines: Option<u64>,
    tokens: Option<usize>,
    tail: bool,
}

fn is_canonical_positive(raw: &str) -> bool {
    !raw.is_empty() && !raw.starts_with('0') && raw.bytes().all(|b| b.is_ascii_digit())
}

fn parse_max_tokens(raw: &str) -> Result<usize, &'static str> {
    if !is_canonical_positive(raw) {
        return Err(MAX_TOKENS_ARGUMENT_ERROR);
    }
    let val = raw.parse::<usize>().unwrap_or(MAX_OUTPUT_TOKENS);
    Ok(val.min(MAX_OUTPUT_TOKENS))
}

fn parse_options(args: &[String]) -> Result<(RunOptions, Vec<String>), String> {
    let mut options = RunOptions {
        lines: None,
        tokens: None,
        tail: true,
    };
    let mut direction_set = false;
    let mut i = 0usize;

    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(val) = arg.strip_prefix("--max-tokens=") {
            if options.tokens.is_some() {
                return Err(MAX_TOKENS_ARGUMENT_ERROR.to_string());
            }
            options.tokens = Some(parse_max_tokens(val).map_err(str::to_string)?);
            i += 1;
            continue;
        }
        match arg {
            "--max-tokens" => {
                if i + 1 >= args.len() || options.tokens.is_some() {
                    return Err(MAX_TOKENS_ARGUMENT_ERROR.to_string());
                }
                options.tokens = Some(parse_max_tokens(&args[i + 1]).map_err(str::to_string)?);
                i += 2;
            }
            "-n" => {
                if i + 1 >= args.len() || options.lines.is_some() {
                    return Err("-n requires one positive integer and cannot repeat".to_string());
                }
                let raw = args[i + 1].as_str();
                if !is_canonical_positive(raw) {
                    return Err("-n requires one positive integer".to_string());
                }
                let parsed = raw
                    .parse::<u64>()
                    .map_err(|_| "-n requires one positive integer".to_string())?;
                options.lines = Some(parsed);
                i += 2;
            }
            "--head" | "--tail" => {
                if direction_set {
                    return Err("specify --head or --tail at most once".to_string());
                }
                direction_set = true;
                options.tail = arg == "--tail";
                i += 1;
            }
            "--" => {
                if (options.tokens.is_none() && options.lines.is_none())
                    || i + 1 >= args.len()
                    || args[i + 1].is_empty()
                {
                    return Err(RUN_USAGE_ERROR.to_string());
                }
                return Ok((options, args[i + 1..].to_vec()));
            }
            _ => {
                if !arg.starts_with('-')
                    && !arg.is_empty()
                    && (options.tokens.is_some() || options.lines.is_some())
                {
                    return Ok((options, args[i..].to_vec()));
                }
                return Err(RUN_USAGE_ERROR.to_string());
            }
        }
    }
    Err(RUN_USAGE_ERROR.to_string())
}

/// Unified bounded head/tail capture window supporting line limits, byte/token limits, or both.
struct OutputWindow {
    max_lines: Option<u64>,
    max_bytes: Option<usize>,
    tail: bool,
    buf: VecDeque<u8>,
    newlines: u64,
    omitted: bool,
}

impl OutputWindow {
    fn new(options: &RunOptions) -> Self {
        let max_bytes = options
            .tokens
            .map(|t| t.saturating_mul(4).saturating_add(4));
        Self {
            max_lines: options.lines,
            max_bytes,
            tail: options.tail,
            buf: VecDeque::new(),
            newlines: 0,
            omitted: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    fn push_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if !self.tail {
            self.push_head(data);
        } else {
            self.push_tail(data);
        }
    }

    fn push_head(&mut self, data: &[u8]) {
        for &byte in data {
            if self.max_lines.is_some_and(|lim| self.newlines >= lim)
                || self.max_bytes.is_some_and(|lim| self.buf.len() >= lim)
            {
                self.omitted = true;
                return;
            }
            self.buf.push_back(byte);
            if byte == b'\n' {
                self.newlines += 1;
            }
        }
    }

    fn push_tail(&mut self, data: &[u8]) {
        if let Some(lim) = self.max_bytes {
            if lim == 0 {
                self.omitted = true;
                return;
            }
            if data.len() >= lim {
                if !self.buf.is_empty() || data.len() > lim {
                    self.omitted = true;
                }
                let slice = &data[data.len() - lim..];
                self.buf.clear();
                self.buf.extend(slice);
                self.newlines = slice.iter().filter(|&&b| b == b'\n').count() as u64;
            } else {
                let excess = (self.buf.len() + data.len()).saturating_sub(lim);
                if excess > 0 {
                    self.omitted = true;
                    for _ in 0..excess {
                        if self.buf.pop_front() == Some(b'\n') {
                            self.newlines -= 1;
                        }
                    }
                }
                self.buf.extend(data);
                self.newlines += data.iter().filter(|&&b| b == b'\n').count() as u64;
            }
        } else {
            self.buf.extend(data);
            self.newlines += data.iter().filter(|&&b| b == b'\n').count() as u64;
        }

        if let Some(max_lines) = self.max_lines {
            loop {
                let trailing_partial =
                    u64::from(!self.buf.is_empty() && self.buf.back() != Some(&b'\n'));
                if self.newlines + trailing_partial <= max_lines || self.newlines == 0 {
                    break;
                }
                self.omitted = true;
                while let Some(byte) = self.buf.pop_front() {
                    if byte == b'\n' {
                        self.newlines -= 1;
                        break;
                    }
                }
            }
        }
    }

    fn finish(mut self, token_budget: Option<usize>) -> (String, bool) {
        let raw = self.buf.make_contiguous();
        let trimmed = if self.omitted && self.max_bytes.is_some() {
            trim_utf8_boundary(raw, self.tail)
        } else {
            &*raw
        };
        let mut text = to_valid_utf8_collapsed(trimmed);
        if let Some(budget) = token_budget {
            let max_bytes = budget.saturating_mul(4);
            if text.len() > max_bytes {
                self.omitted = true;
                if max_bytes == 0 {
                    text.clear();
                } else if self.tail {
                    let start = text.ceil_char_boundary(text.len() - max_bytes);
                    text.drain(..start);
                } else {
                    let end = text.floor_char_boundary(max_bytes);
                    text.truncate(end);
                }
            }
        }
        (text, self.omitted)
    }
}

fn trim_utf8_boundary(bytes: &[u8], tail: bool) -> &[u8] {
    if tail {
        let start = bytes
            .iter()
            .position(|&b| (b & 0b1100_0000) != 0b1000_0000)
            .unwrap_or(bytes.len());
        &bytes[start..]
    } else {
        let mut end = bytes.len();
        while end > 0 && bytes[end - 1] >= 0x80 {
            let check_start = end.saturating_sub(4);
            let mut probe = check_start;
            while probe < end && (bytes[probe] & 0b1100_0000) == 0b1000_0000 {
                probe += 1;
            }
            if std::str::from_utf8(&bytes[probe..end]).is_ok() {
                break;
            }
            end -= 1;
        }
        &bytes[..end]
    }
}

fn approx_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn spawn_pipe_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: Arc<Mutex<Option<OutputWindow>>>,
) -> (std::sync::mpsc::Receiver<()>, std::thread::JoinHandle<()>) {
    crate::common::spawn_bounded_reader(move || {
        if let Some(mut pipe) = pipe {
            let mut buf = [0u8; 32 * 1024];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut guard) = cap.lock()
                    && let Some(window) = guard.as_mut()
                {
                    window.push_bytes(&buf[..n]);
                }
            }
        }
    })
}

fn exit_code_from_status(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

#[cfg(unix)]
mod tty_handoff {
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Child;
    use std::process::Command;
    use std::process::ExitStatus;
    use std::time::Duration;

    /// Run `f` with `sig` blocked on the calling thread, then restore the previous mask.
    /// Blocking `SIGTTOU` allows background `tcsetpgrp`; blocking a stop signal around
    /// `kill(-pgrp, sig)` ensures synchronous stop delivery on this thread upon unblock.
    fn with_signal_blocked(sig: libc::c_int, f: impl FnOnce()) {
        // SAFETY: Stack-allocated `sigset_t` values with thread-local `pthread_sigmask`.
        unsafe {
            let mut block_set: libc::sigset_t = std::mem::zeroed();
            let mut old_set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut block_set);
            libc::sigaddset(&mut block_set, sig);
            let masked = libc::pthread_sigmask(libc::SIG_BLOCK, &block_set, &mut old_set) == 0;
            f();
            if masked {
                let _ = libc::pthread_sigmask(libc::SIG_SETMASK, &old_set, std::ptr::null_mut());
            }
        }
    }

    fn set_foreground_pgrp(tty_fd: libc::c_int, pgrp: libc::pid_t) {
        with_signal_blocked(libc::SIGTTOU, || {
            // SAFETY: `tty_fd` is the controlling terminal and `pgrp` is a valid process group.
            unsafe {
                let _ = libc::tcsetpgrp(tty_fd, pgrp);
            }
        });
    }

    pub(super) struct TerminalForegroundGuard {
        tty_fd: libc::c_int,
        original_pgrp: libc::pid_t,
        child_pgrp: Option<libc::pid_t>,
    }

    impl TerminalForegroundGuard {
        /// Capture the caller's process group when stdin is our controlling terminal and we
        /// currently own its foreground process group.
        pub(super) fn capture_if_foreground() -> Option<Self> {
            let tty_fd = libc::STDIN_FILENO;
            // SAFETY: `isatty`, `getpgrp`, and `tcgetpgrp` are thread-safe POSIX queries.
            unsafe {
                if libc::isatty(tty_fd) != 1 {
                    return None;
                }
                let current_pgrp = libc::getpgrp();
                if current_pgrp <= 0 || libc::tcgetpgrp(tty_fd) != current_pgrp {
                    return None;
                }
                Some(Self {
                    tty_fd,
                    original_pgrp: current_pgrp,
                    child_pgrp: None,
                })
            }
        }

        pub(super) fn set_child_pgrp(&mut self, child_pid: u32) {
            self.child_pgrp = Some(child_pid as libc::pid_t);
        }

        fn restore_if_owned(&self) -> bool {
            // SAFETY: `tcgetpgrp` is a thread-safe POSIX query on `tty_fd`.
            let fg_pgrp = unsafe { libc::tcgetpgrp(self.tty_fd) };
            let should_restore = match self.child_pgrp {
                Some(child_pgrp) => fg_pgrp == child_pgrp,
                None => fg_pgrp != self.original_pgrp,
            };
            if should_restore {
                set_foreground_pgrp(self.tty_fd, self.original_pgrp);
            }
            should_restore
        }

        fn handoff_to_if_foreground(&self, child_pgrp: libc::pid_t) -> bool {
            // SAFETY: `tcgetpgrp` is a thread-safe POSIX query on `tty_fd`.
            let is_fg = unsafe { libc::tcgetpgrp(self.tty_fd) } == self.original_pgrp;
            if is_fg {
                set_foreground_pgrp(self.tty_fd, child_pgrp);
            }
            is_fg
        }

        /// Wait for the child while forwarding Ctrl-Z (`SIGTSTP`) stops to `original_pgrp` and
        /// restoring child foreground ownership on `fg` (including after `bg`).
        pub(super) fn wait_child(&self, child: &mut Child) -> std::io::Result<ExitStatus> {
            let child_pid = child.id() as libc::pid_t;
            let mut handed_off = true;
            loop {
                if !handed_off {
                    handed_off = self.handoff_to_if_foreground(child_pid);
                }
                let mut raw_status: libc::c_int = 0;
                let flags = if handed_off {
                    libc::WUNTRACED
                } else {
                    libc::WUNTRACED | libc::WNOHANG
                };
                // SAFETY: `child_pid` is a direct unwaited child and `raw_status` is valid.
                let waited = unsafe { libc::waitpid(child_pid, &mut raw_status, flags) };
                if waited == 0 {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                if waited == -1 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(err);
                }
                if libc::WIFSTOPPED(raw_status) {
                    let stop_sig = libc::WSTOPSIG(raw_status);
                    let _ = self.restore_if_owned();
                    with_signal_blocked(stop_sig, || {
                        // SAFETY: `original_pgrp > 0` and `stop_sig` came from `waitpid`.
                        unsafe {
                            libc::kill(-self.original_pgrp, stop_sig);
                        }
                    });
                    handed_off = self.handoff_to_if_foreground(child_pid);
                    // SAFETY: `child_pid > 0` is the child's process group leader.
                    unsafe {
                        libc::kill(-child_pid, libc::SIGCONT);
                    }
                    continue;
                }
                if libc::WIFEXITED(raw_status) || libc::WIFSIGNALED(raw_status) {
                    return Ok(ExitStatus::from_raw(raw_status));
                }
            }
        }

        /// Restore foreground ownership if still held by the child, and propagate terminal
        /// interrupt signals (`SIGINT` / `SIGQUIT`) back to `original_pgrp`.
        pub(super) fn finish(self, status: ExitStatus) {
            let restored = self.restore_if_owned();
            let original_pgrp = self.original_pgrp;
            std::mem::forget(self);
            if restored
                && let Some(sig) = status.signal()
                && (sig == libc::SIGINT || sig == libc::SIGQUIT)
            {
                // SAFETY: `original_pgrp > 0` was just restored as the foreground group.
                unsafe {
                    libc::kill(-original_pgrp, sig);
                }
            }
        }
    }

    impl Drop for TerminalForegroundGuard {
        fn drop(&mut self) {
            let _ = self.restore_if_owned();
        }
    }

    /// Place the child into its own process group and, when `mrun` owns the controlling
    /// terminal foreground, hand foreground ownership to the child before `exec` so reading
    /// from PTY stdin does not trigger `SIGTTIN`.
    pub(super) fn configure_child_process_group(
        cmd: &mut Command,
        tty_guard: Option<&TerminalForegroundGuard>,
    ) {
        let handoff_tty_fd = tty_guard.map(|guard| guard.tty_fd);
        // SAFETY: Runs in the single-threaded post-`fork` child before `exec` and invokes only
        // POSIX async-signal-safe functions.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setpgid(0, 0) == 0
                    && let Some(tty_fd) = handoff_tty_fd
                {
                    let mut block_set: libc::sigset_t = std::mem::zeroed();
                    let mut old_set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut block_set);
                    libc::sigaddset(&mut block_set, libc::SIGTTOU);
                    let masked = libc::sigprocmask(libc::SIG_BLOCK, &block_set, &mut old_set) == 0;
                    let _ = libc::tcsetpgrp(tty_fd, libc::getpid());
                    if masked {
                        let _ =
                            libc::sigprocmask(libc::SIG_SETMASK, &old_set, std::ptr::null_mut());
                    }
                }
                Ok(())
            });
        }
    }
}

#[cfg(not(unix))]
mod tty_handoff {
    use std::process::Child;
    use std::process::Command;
    use std::process::ExitStatus;

    pub(super) struct TerminalForegroundGuard;

    impl TerminalForegroundGuard {
        pub(super) fn capture_if_foreground() -> Option<Self> {
            None
        }
        pub(super) fn set_child_pgrp(&mut self, _child_pid: u32) {}
        pub(super) fn wait_child(&self, child: &mut Child) -> std::io::Result<ExitStatus> {
            child.wait()
        }
        pub(super) fn finish(self, _status: ExitStatus) {}
    }

    pub(super) fn configure_child_process_group(
        cmd: &mut Command,
        _tty_guard: Option<&TerminalForegroundGuard>,
    ) {
        crate::common::configure_new_process_group(cmd);
    }
}

pub fn execute(args: &[String]) -> ToolOutput {
    let (options, argv) = match parse_options(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!(
                    "mrun: {err}
"
                ),
                exit_code: 2,
            };
        }
    };

    let resolved = match which::which(&argv[0]) {
        Ok(path) => path,
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!(
                    "{}: {err}
",
                    argv[0]
                ),
                exit_code: 127,
            };
        }
    };

    let mut cmd = Command::new(&resolved);
    cmd.args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tty_guard = tty_handoff::TerminalForegroundGuard::capture_if_foreground();
    tty_handoff::configure_child_process_group(&mut cmd, tty_guard.as_ref());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let code = if err.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                1
            };
            return ToolOutput {
                stdout: String::new(),
                stderr: format!(
                    "{}: {err}
",
                    argv[0]
                ),
                exit_code: code,
            };
        }
    };
    if let Some(guard) = tty_guard.as_mut() {
        guard.set_child_pgrp(child.id());
    }

    let out_cap = Arc::new(Mutex::new(Some(OutputWindow::new(&options))));
    let err_cap = Arc::new(Mutex::new(Some(OutputWindow::new(&options))));
    let (out_rx, out_handle) = spawn_pipe_reader(child.stdout.take(), Arc::clone(&out_cap));
    let (err_rx, err_handle) = spawn_pipe_reader(child.stderr.take(), Arc::clone(&err_cap));

    let wait_result = match tty_guard.as_ref() {
        Some(guard) => guard.wait_child(&mut child),
        None => child.wait(),
    };
    let status = match wait_result {
        Ok(status) => status,
        Err(err) => {
            crate::common::retire_child_process_group(&mut child);
            return ToolOutput {
                stdout: String::new(),
                stderr: format!(
                    "mrun: wait child: {err}
"
                ),
                exit_code: 1,
            };
        }
    };

    // Terminate any descendants still holding inherited stdout/stderr pipes so draining cannot hang.
    crate::common::kill_process_group(child.id() as i32);
    if let Some(guard) = tty_guard {
        guard.finish(status);
    }
    let drain_timeout = std::time::Duration::from_secs(2);
    let _ = crate::common::recv_reader_with_timeout(out_rx, out_handle, drain_timeout);
    let _ = crate::common::recv_reader_with_timeout(err_rx, err_handle, drain_timeout);

    let out_window = out_cap
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .unwrap_or_else(|| OutputWindow::new(&options));
    let err_window = err_cap
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .unwrap_or_else(|| OutputWindow::new(&options));

    let (selected_out, out_omitted, selected_err, err_omitted) =
        if let Some(budget) = options.tokens {
            // Reserve at most half the token budget for stderr when stdout is non-empty.
            let err_budget = if out_window.is_empty() {
                budget
            } else {
                budget / 2
            };
            let (sel_err, err_omitted) = err_window.finish(Some(err_budget));
            let out_budget = budget.saturating_sub(approx_tokens(&sel_err));
            let (sel_out, out_omitted) = out_window.finish(Some(out_budget));
            (sel_out, out_omitted, sel_err, err_omitted)
        } else {
            let (sel_out, out_omitted) = out_window.finish(None);
            let (sel_err, err_omitted) = err_window.finish(None);
            (sel_out, out_omitted, sel_err, err_omitted)
        };

    let mut final_stderr = selected_err;
    if out_omitted || err_omitted {
        let limit_desc = match (options.lines, options.tokens) {
            (Some(lines), Some(tokens)) => format!("{lines}-line limit or {tokens}-token limit"),
            (Some(lines), None) => format!("{lines}-line limit"),
            (None, Some(tokens)) => format!("{tokens}-token limit"),
            (None, None) => String::new(),
        };
        if !final_stderr.is_empty() && !final_stderr.ends_with('\n') {
            final_stderr.push('\n');
        }
        final_stderr.push_str(&format!("mrun: output incomplete: {limit_desc} reached\n"));
    }

    ToolOutput {
        stdout: selected_out,
        stderr: final_stderr,
        exit_code: exit_code_from_status(status),
    }
}

#[cfg(test)]
mod tests;
