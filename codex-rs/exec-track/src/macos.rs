//! macOS `DYLD_INSERT_LIBRARIES` process-scoped write observer.
//! Embeds the compiled `libmekugi_write_observer.dylib` interposer, serves
//! pre-write baseline notifications over a private Unix socket, prefers a
//! non-SIP shell when available, and explicitly marks SIP-protected fallbacks
//! as `CoverageStatus::Partial`.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

use crate::CODEX_EXEC_OBSERVE_ARG1;
use crate::WrappedCommand;
use crate::ipc::serve_unix_message;
use crate::persist::ObservationState;

const INTERPOSE_DYLIB: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/libmekugi_write_observer.dylib"));
const ENV_SOCK: &str = "MEKUGI_WRITE_OBSERVER_SOCK";

pub(crate) struct MacosObserver {
    sock_path: PathBuf,
    dylib_path: PathBuf,
    state: Arc<Mutex<ObservationState>>,
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl MacosObserver {
    pub(crate) fn start(dir: &Path, state: Arc<Mutex<ObservationState>>) -> io::Result<Self> {
        let dylib_path = dir.join("libmekugi_write_observer.dylib");
        fs::write(&dylib_path, INTERPOSE_DYLIB)?;
        fs::set_permissions(&dylib_path, fs::Permissions::from_mode(0o755))?;

        let sock_path = dir.join("observe.sock");
        let listener = UnixListener::bind(&sock_path)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let state_bg = Arc::clone(&state);
        let handle = thread::Builder::new()
            .name("mekugi-macos-observer".to_string())
            .spawn(move || {
                while !stop_bg.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            serve_unix_message(stream, &state_bg);
                        }
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                            let mut pfd = libc::pollfd {
                                fd: listener.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            };
                            unsafe { libc::poll(&mut pfd, 1, 25) };
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            sock_path,
            dylib_path,
            state,
            stop,
            handle: Mutex::new(Some(handle)),
        })
    }

    pub(crate) fn wrap_command(
        &self,
        argv: &[String],
        self_exe: Option<&Path>,
    ) -> Option<WrappedCommand> {
        if argv.is_empty() {
            return None;
        }
        let mut new_argv = argv.to_vec();
        if is_sip_path(&new_argv[0]) {
            if let Some(alt_shell) = find_non_sip_shell(&new_argv[0]) {
                new_argv[0] = alt_shell;
            } else {
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record_partial_reason(format!(
                        "SIP-protected shell prevents DYLD_INSERT_LIBRARIES: {}",
                        new_argv[0]
                    ));
            }
        }
        if let Some(exe) = self_exe {
            let mut wrapped = Vec::with_capacity(new_argv.len() + 5);
            wrapped.push(exe.to_string_lossy().into_owned());
            wrapped.push(CODEX_EXEC_OBSERVE_ARG1.to_string());
            wrapped.push(self.sock_path.to_string_lossy().into_owned());
            wrapped.push(self.dylib_path.to_string_lossy().into_owned());
            wrapped.push("--".to_string());
            wrapped.extend(new_argv);
            return Some(WrappedCommand {
                argv: wrapped,
                env: Vec::new(),
            });
        }
        let existing = std::env::var("DYLD_INSERT_LIBRARIES").unwrap_or_default();
        let dyld_val = if existing.is_empty() {
            self.dylib_path.to_string_lossy().into_owned()
        } else {
            format!("{}:{existing}", self.dylib_path.display())
        };
        Some(WrappedCommand {
            argv: new_argv,
            env: vec![
                ("DYLD_INSERT_LIBRARIES".to_string(), dyld_val),
                (
                    ENV_SOCK.to_string(),
                    self.sock_path.to_string_lossy().into_owned(),
                ),
            ],
        })
    }

    pub(crate) fn stop_and_join(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
    }
}

fn is_sip_path(path: &str) -> bool {
    path.starts_with("/bin/")
        || path.starts_with("/usr/bin/")
        || path.starts_with("/sbin/")
        || path.starts_with("/usr/sbin/")
}

fn find_non_sip_shell(original: &str) -> Option<String> {
    let name = Path::new(original).file_name()?.to_str()?;
    let candidates: &[&str] = match name {
        "bash" => &["/opt/homebrew/bin/bash", "/usr/local/bin/bash"],
        "zsh" => &["/opt/homebrew/bin/zsh", "/usr/local/bin/zsh"],
        "sh" => &[
            "/opt/homebrew/bin/dash",
            "/opt/homebrew/bin/bash",
            "/usr/local/bin/bash",
        ],
        _ => return None,
    };
    candidates
        .iter()
        .find(|p| Path::new(p).is_file())
        .map(|p| (*p).to_string())
}

/// Child wrapper entrypoint on macOS invoked via
/// `<codex-exe> --codex-exec-observe <sock> <dylib> -- <cmd> [args...]`.
/// Sets `DYLD_INSERT_LIBRARIES` and `MEKUGI_WRITE_OBSERVER_SOCK` inside the
/// Seatbelt sandbox (after SIP-protected `sandbox-exec` has launched) and `execvp`s `<cmd>`.
pub fn run_observe_wrapper_main() -> ! {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let Some(sep) = args.iter().position(|a| a == "--") else {
        std::process::exit(126);
    };
    let cmd_args = &args[sep + 1..];
    if cmd_args.is_empty() {
        std::process::exit(126);
    }
    if let (Some(sock), Some(dylib)) = (args.get(2), args.get(3)) {
        let existing = std::env::var_os("DYLD_INSERT_LIBRARIES").unwrap_or_default();
        let mut dyld_val = dylib.clone();
        if !existing.is_empty() {
            dyld_val.push(":");
            dyld_val.push(existing);
        }
        unsafe {
            std::env::set_var("DYLD_INSERT_LIBRARIES", dyld_val);
            std::env::set_var(ENV_SOCK, sock);
        }
    }
    let c_args: Vec<CString> = cmd_args
        .iter()
        .filter_map(|a| CString::new(a.as_bytes()).ok())
        .collect();
    if c_args.is_empty() {
        std::process::exit(126);
    }
    let mut ptrs: Vec<*const libc::c_char> = c_args.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe {
        libc::execvp(ptrs[0], ptrs.as_ptr());
    }
    std::process::exit(127);
}
