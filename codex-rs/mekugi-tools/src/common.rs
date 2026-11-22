use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

/// Maximum safe JavaScript integer (`2^53 - 1`), matching Go `commandargs`.
pub const MAX_SAFE_INT: u64 = (1u64 << 53) - 1;

/// Quote a path as a JSON string, escaping control characters cleanly.
pub fn quote_json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

/// Quote a path for bundle headers/manifests, also escaping JS line/paragraph separators.
pub fn quote_bundle_path(path: &str) -> String {
    quote_json_string(path)
        .replace('\u{2028}', r"\u2028")
        .replace('\u{2029}', r"\u2029")
}

/// Render a compact Bash-safe argument matching Go's `commandargs.QuoteArgument`.
pub fn quote_shell_arg(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let is_plain = value.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'/' | b'.' | b'_' | b'-' | b':' | b'+' | b'=' | b'@' | b',' | b'%'
            )
    });
    if is_plain {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Open a file for reading without blocking on FIFOs on Unix.
pub fn open_nonblocking(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    opts.open(path)
}

/// Lexically clean a path like Go's `filepath.Clean` without touching the filesystem.
pub fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if let Some(&last) = stack.last() {
                    if last == ".." {
                        stack.push("..");
                    } else {
                        stack.pop();
                    }
                } else if !rooted {
                    stack.push("..");
                }
            }
            part => stack.push(part),
        }
    }
    if stack.is_empty() {
        return if rooted {
            "/".to_string()
        } else {
            ".".to_string()
        };
    }
    let joined = stack.join("/");
    if rooted { format!("/{joined}") } else { joined }
}

/// Convert a path to slash-separated UTF-8 representation.
pub fn path_to_slash(path: &Path) -> String {
    let s = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '\\' {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

/// Return true if `path` is outside `root` (both cleaned/canonicalized).
pub fn outside_workspace(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_err()
}

/// Return a relative path from `root` to `path` if `path` is inside `root`.
pub fn relative_to_root(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Format an I/O error message matching Go's `sourceFailure` conventions for `mcat`.
pub fn format_io_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "ENOENT: no such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "EACCES: permission denied".to_string(),
        io::ErrorKind::NotADirectory => "ENOTDIR: not a directory".to_string(),
        _ => err.to_string(),
    }
}

/// Convert arbitrary bytes to UTF-8, collapsing each consecutive invalid-byte run into one U+FFFD
/// (matching Go's `strings.ToValidUTF8(value, "\uFFFD")`).
pub fn to_valid_utf8_collapsed(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    let mut in_invalid = false;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(err) => {
                let valid_len = err.valid_up_to();
                if valid_len > 0 {
                    // Safety: `valid_up_to` guarantees valid UTF-8 prefix.
                    if let Ok(prefix) = std::str::from_utf8(&rest[..valid_len]) {
                        out.push_str(prefix);
                    }
                    in_invalid = false;
                }
                if !in_invalid {
                    out.push('\u{FFFD}');
                    in_invalid = true;
                }
                let advance = err.error_len().unwrap_or(rest.len() - valid_len);
                rest = &rest[valid_len + advance..];
            }
        }
    }
    out
}

/// Place a child command into its own Unix process group (`setpgid(0, 0)`) so teardown
/// can terminate the entire descendant tree cleanly on both Linux and macOS.
pub fn configure_new_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Send `SIGKILL` to the child's entire Unix process group (`kill(-pid, SIGKILL)`).
pub fn kill_process_group(pid: i32) {
    #[cfg(unix)]
    if pid > 0 {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Kill the child's entire process group and reap the child process.
pub fn retire_child_process_group(child: &mut Child) {
    let pid = child.id() as i32;
    kill_process_group(pid);
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawn a reader thread whose result is delivered over a bounded `sync_channel(1)` so
/// callers can enforce a bounded drain timeout without hanging on inherited-pipe descendants.
pub fn spawn_bounded_reader<T, F>(f: F) -> (mpsc::Receiver<T>, std::thread::JoinHandle<()>)
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    let handle = std::thread::spawn(move || {
        let value = f();
        let _ = tx.send(value);
    });
    (rx, handle)
}

/// Wait up to `timeout` for a reader thread spawned via [`spawn_bounded_reader`].
/// If the result arrives in time, join the thread immediately; otherwise return `None`
/// without blocking forever on a leaked pipe descriptor.
pub fn recv_reader_with_timeout<T>(
    rx: mpsc::Receiver<T>,
    handle: std::thread::JoinHandle<()>,
    timeout: Duration,
) -> Option<T> {
    match rx.recv_timeout(timeout) {
        Ok(value) => {
            let _ = handle.join();
            Some(value)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = handle.join();
            None
        }
        Err(mpsc::RecvTimeoutError::Timeout) => None,
    }
}
