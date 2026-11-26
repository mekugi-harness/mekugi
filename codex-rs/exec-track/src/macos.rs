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
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
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
const SHIMS_DIR_NAME: &str = "shims";
const SHIM_UTILITIES: &[&str] = &[
    "sh", "bash", "zsh", "sed", "awk", "cp", "mv", "rm", "install", "touch", "tee", "patch",
];
static CACHED_SHIMS_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

pub(crate) struct MacosObserver {
    sock_path: PathBuf,
    dylib_path: PathBuf,
    shims_dir: Option<PathBuf>,
    state: Arc<Mutex<ObservationState>>,
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl MacosObserver {
    pub(crate) fn start(dir: &Path, state: Arc<Mutex<ObservationState>>) -> io::Result<Self> {
        let dylib_path = dir.join("libmekugi_write_observer.dylib");
        fs::write(&dylib_path, INTERPOSE_DYLIB)?;
        fs::set_permissions(&dylib_path, fs::Permissions::from_mode(0o755))?;
        let shims_dir = cached_non_sip_shims_dir();

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
            shims_dir,
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
            if let Some(alt_shell) = find_non_sip_shell(&new_argv[0], self.shims_dir.as_deref()) {
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
            let mut wrapped = Vec::with_capacity(new_argv.len() + 6);
            wrapped.push(exe.to_string_lossy().into_owned());
            wrapped.push(CODEX_EXEC_OBSERVE_ARG1.to_string());
            wrapped.push(self.sock_path.to_string_lossy().into_owned());
            wrapped.push(self.dylib_path.to_string_lossy().into_owned());
            if let Some(shims) = &self.shims_dir {
                wrapped.push(shims.to_string_lossy().into_owned());
            }
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

    pub(crate) fn shims_dir(&self) -> Option<&Path> {
        self.shims_dir.as_deref()
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

fn non_sip_utility_candidates(name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(format!("/opt/homebrew/bin/{name}")),
        PathBuf::from(format!("/usr/local/bin/{name}")),
        PathBuf::from(format!("/run/current-system/sw/bin/{name}")),
        PathBuf::from(format!("/nix/var/nix/profiles/default/bin/{name}")),
    ];
    if name == "sh" {
        candidates.insert(0, PathBuf::from("/opt/homebrew/bin/dash"));
        candidates.push(PathBuf::from("/opt/homebrew/bin/bash"));
        candidates.push(PathBuf::from("/usr/local/bin/bash"));
    }
    candidates
}

fn cached_non_sip_shims_dir() -> Option<PathBuf> {
    CACHED_SHIMS_DIR
        .get_or_init(|| {
            let base = std::env::var_os("CODEX_ARG0_PATH_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::temp_dir().join(format!("mekugi-non-sip-{}", std::process::id()))
                });
            stage_non_sip_shims(&base.join(SHIMS_DIR_NAME))
        })
        .clone()
}

/// Stages a non-SIP copy of a system binary only when it can actually execute
/// under ad-hoc signing (Apple Silicon kills ad-hoc-signed `arm64e` slices
/// with SIGKILL/137 via AMFI).
fn stage_signed_system_copy(name: &str, src: &Path, dst: &Path) -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        let archs_out = std::process::Command::new("/usr/bin/lipo")
            .arg("-archs")
            .arg(src)
            .output();
        let Ok(out) = archs_out else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        let archs = String::from_utf8_lossy(&out.stdout);
        let tokens: Vec<&str> = archs.split_whitespace().collect();
        if tokens.contains(&"arm64") {
            let extracted = std::process::Command::new("/usr/bin/lipo")
                .arg(src)
                .args(["-thin", "arm64", "-output"])
                .arg(dst)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !extracted {
                let _ = fs::remove_file(dst);
                return false;
            }
        } else {
            // System binary only provides arm64e (no plain arm64 slice); ad-hoc
            // signing it triggers AMFI SIGKILL on Apple Silicon.
            return false;
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        if fs::copy(src, dst).is_err() {
            return false;
        }
    }

    if fs::set_permissions(dst, fs::Permissions::from_mode(0o755)).is_err() {
        let _ = fs::remove_file(dst);
        return false;
    }
    // Ad-hoc signing strips Apple's TF_PLATFORM flag so Darwin preserves DYLD_INSERT_LIBRARIES.
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(dst)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !signed {
        let _ = fs::remove_file(dst);
        return false;
    }
    // Verify the staged binary is not killed by AMFI (e.g. SIGKILL/137) before exposing it.
    let mut probe = std::process::Command::new(dst);
    if matches!(name, "sh" | "bash" | "zsh") {
        probe.args(["-c", "true"]);
    } else {
        probe.arg("--version");
    }
    let executable_ok = probe
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.signal().is_none());
    if !executable_ok {
        let _ = fs::remove_file(dst);
        return false;
    }
    true
}

fn stage_non_sip_shims(shims_dir: &Path) -> Option<PathBuf> {
    fs::create_dir_all(shims_dir).ok()?;
    let mut staged = 0usize;
    for name in SHIM_UTILITIES {
        let dst = shims_dir.join(name);
        if dst.is_file() {
            staged += 1;
            continue;
        }
        if let Some(src) = non_sip_utility_candidates(name)
            .into_iter()
            .find(|p| p.is_file())
        {
            if std::os::unix::fs::symlink(&src, &dst).is_ok() {
                staged += 1;
                continue;
            }
        }
        let system_src = [format!("/bin/{name}"), format!("/usr/bin/{name}")]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file());
        let Some(src) = system_src else {
            continue;
        };
        if stage_signed_system_copy(name, &src, &dst) {
            staged += 1;
        }
    }
    (staged > 0).then(|| shims_dir.to_path_buf())
}

fn find_non_sip_shell(original: &str, shims_dir: Option<&Path>) -> Option<String> {
    let name = Path::new(original).file_name()?.to_str()?;
    if let Some(dir) = shims_dir {
        let shimmed = dir.join(name);
        if shimmed.is_file() {
            return Some(shimmed.to_string_lossy().into_owned());
        }
    }
    if !matches!(name, "bash" | "zsh" | "sh") {
        return None;
    }
    non_sip_utility_candidates(name)
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

fn is_sip_dir(dir: &Path) -> bool {
    ["/bin", "/usr/bin", "/sbin", "/usr/sbin"]
        .into_iter()
        .any(|path| dir == Path::new(path))
}

/// Insert `shims_dir` immediately before the first SIP system directory on `PATH`
/// so user-configured utilities earlier on `PATH` (e.g. Homebrew GNU sed) are never shadowed.
fn inject_shims_before_sip_dirs(existing_path: &std::ffi::OsStr, shims_dir: &Path) -> std::ffi::OsString {
    let mut entries: Vec<PathBuf> = std::env::split_paths(existing_path).collect();
    if entries.iter().any(|p| p == shims_dir) {
        return existing_path.to_os_string();
    }
    if let Some(pos) = entries.iter().position(|p| is_sip_dir(p)) {
        entries.insert(pos, shims_dir.to_path_buf());
    } else {
        entries.push(shims_dir.to_path_buf());
    }
    std::env::join_paths(entries).unwrap_or_else(|_| existing_path.to_os_string())
}

/// Child wrapper entrypoint on macOS invoked via
/// `<codex-exe> --codex-exec-observe <sock> <dylib> [shims_dir] -- <cmd> [args...]`.
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
        let shims_candidate = (sep > 4).then(|| PathBuf::from(&args[4]));
        if let Some(shims_dir) = shims_candidate
            && shims_dir.is_dir()
        {
            let existing_path = std::env::var_os("PATH").unwrap_or_default();
            let new_path = inject_shims_before_sip_dirs(&existing_path, &shims_dir);
            unsafe {
                std::env::set_var("PATH", new_path);
            }
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
