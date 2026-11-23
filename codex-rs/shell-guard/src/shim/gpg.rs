use crate::classifier::gpg;
use crate::protocol::ARG1_GPG_RUN;
use crate::protocol::GuardKind;
use crate::protocol::MAX_MESSAGE_BYTES;
use crate::protocol::ShimRequest;
use crate::shim::ShimContext;
use crate::shim::client::send_request;
use crate::shim::paths::guard_paths;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

const OPTION_TABLE_TIMEOUT: Duration = Duration::from_secs(5);
static FORWARD_CHILD_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_signal_handler(sig: libc::c_int) {
    let pid = FORWARD_CHILD_PID.load(Ordering::Relaxed);
    if pid > 0 {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

fn install_signal_forwarder(child_pid: u32) {
    FORWARD_CHILD_PID.store(child_pid as i32, Ordering::SeqCst);
    for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = forward_signal_handler as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
}

fn clear_signal_forwarder() {
    FORWARD_CHILD_PID.store(0, Ordering::SeqCst);
}

/// Runs native GPG while intercepting `[GNUPG:] GET_HIDDEN passphrase.enter`
/// over private loopback `--status-fd` / `--command-fd` pipes, preserving all
/// caller-inherited descriptors and exit status.
pub(crate) fn run_gpg(
    ctx: &ShimContext,
    target: &str,
    argv0: &str,
    default_path: bool,
    args: &[OsString],
) -> i32 {
    let name = Path::new(target)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(target);
    let explicit = target.contains('/').then_some(target);
    let (_guard_dir, real) = match guard_paths(name, explicit, default_path) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("gpg: {err}");
            return 127;
        }
    };

    let options = match dump_option_table(&real) {
        Ok(opts) => opts,
        Err(_) => {
            eprintln!("mekugi: GPG option metadata unavailable; retaining native input");
            return exec_native(&real, argv0, args);
        }
    };
    let parsed = gpg::parse_arguments(args, &options);
    if !parsed.interactive {
        return exec_native(&real, argv0, args);
    }

    // If caller specified `--status-fd >= 3`, verify that FD is actually open
    // and non-CLOEXEC before allocating our own pipes so a closed caller FD
    // cannot be aliased by one of our new pipes.
    if let Some(status_fd) = parsed.status_fd
        && status_fd >= 3
        && !is_open_inheritable_fd(status_fd)
    {
        return exec_native(&real, argv0, args);
    }

    let Ok((status_read, status_write)) = make_pipe() else {
        return 1;
    };
    let Ok((answer_read, answer_write)) = make_pipe() else {
        return 1;
    };
    let Ok((lifetime_read, lifetime_write)) = make_pipe() else {
        return 1;
    };

    // Only the child ends should survive exec into the supervisor process.
    if clear_cloexec(status_write.as_raw_fd()).is_err()
        || clear_cloexec(answer_read.as_raw_fd()).is_err()
        || clear_cloexec(lifetime_read.as_raw_fd()).is_err()
    {
        return 1;
    }

    let Ok(helper) = std::env::current_exe() else {
        return 1;
    };
    let supervisor_args = [
        ARG1_GPG_RUN.to_owned(),
        lifetime_read.as_raw_fd().to_string(),
        real.to_string_lossy().into_owned(),
        argv0.to_owned(),
        "--pinentry-mode".to_owned(),
        "loopback".to_owned(),
        "--status-fd".to_owned(),
        status_write.as_raw_fd().to_string(),
        "--command-fd".to_owned(),
        answer_read.as_raw_fd().to_string(),
    ];

    let mut child = match Command::new(&helper)
        .args(supervisor_args)
        .args(parsed.forward)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return 126,
    };

    install_signal_forwarder(child.id());
    drop(status_write);
    drop(answer_read);
    drop(lifetime_read);

    let mut answer_writer = answer_write;
    let mut caller_status = parsed.status_fd.map(|fd| {
        // SAFETY: we do not close the caller's FD at the end; we use `ManuallyDrop`
        // to write forwarded status lines without taking ownership.
        std::mem::ManuallyDrop::new(unsafe { File::from_raw_fd(fd) })
    });

    let mut reader = BufReader::new(status_read);
    let mut line_buf = Vec::new();
    let failed = loop {
        line_buf.clear();
        let read = match (&mut reader)
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line_buf)
        {
            Ok(n) => n,
            Err(_) => break true,
        };
        if read == 0 {
            break false;
        }
        if line_buf.len() > MAX_MESSAGE_BYTES {
            break true;
        }
        if let Some(out) = caller_status.as_mut()
            && out.write_all(&line_buf).is_err()
        {
            break true;
        }
        let line = String::from_utf8_lossy(&line_buf);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !trimmed.starts_with("[GNUPG:] GET_") {
            continue;
        }
        if trimmed != "[GNUPG:] GET_HIDDEN passphrase.enter" {
            eprintln!("mekugi: GPG non-passphrase inquiry requires caller-owned --command-fd");
            break true;
        }
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let req = ShimRequest {
            kind: GuardKind::GpgPassphrase,
            thread_id: ctx.thread_id.clone(),
            item_id: ctx.item_id.clone(),
            cwd,
            argv: vec![real.to_string_lossy().into_owned()],
            executable: real.to_string_lossy().into_owned(),
        };
        let passphrase = match send_request(ctx.socket_path.as_deref(), &req) {
            Ok(reply) if reply.ok => reply.secret.filter(|s| gpg::valid_passphrase(s.as_str())),
            _ => None,
        };
        let Some(passphrase) = passphrase else {
            break true;
        };
        if answer_writer
            .write_all(passphrase.as_str().as_bytes())
            .is_err()
            || answer_writer.write_all(b"\n").is_err()
            || answer_writer.flush().is_err()
        {
            break true;
        }
        drop(passphrase);
    };

    drop(answer_writer);
    drop(lifetime_write);

    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => {
            clear_signal_forwarder();
            return 1;
        }
    };
    clear_signal_forwarder();
    if failed { 1 } else { gpg_exit(status) }
}

/// Private supervisor process that owns and reaps GPG. If its parent dies (even on
/// SIGKILL), reading the lifetime pipe hits EOF and immediately terminates GPG.
pub(crate) fn run_gpg_supervisor(raw_args: &[OsString]) -> i32 {
    if raw_args.len() < 3 {
        return 1;
    }
    let Ok(lifetime_fd) = raw_args[0].to_string_lossy().parse::<RawFd>() else {
        return 1;
    };
    if lifetime_fd < 3 {
        return 1;
    }
    if set_cloexec(lifetime_fd).is_err() {
        return 1;
    }
    // SAFETY: `lifetime_fd` was exclusively inherited from `run_gpg`.
    let mut parent_pipe = unsafe { File::from_raw_fd(lifetime_fd) };
    let executable = &raw_args[1];
    let argv0 = &raw_args[2];
    let gpg_args = &raw_args[3..];

    let mut child = match Command::new(executable)
        .arg0(argv0)
        .args(gpg_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return 126,
    };

    let child_pid = child.id() as i32;
    install_signal_forwarder(child.id());

    let _watcher = std::thread::spawn(move || {
        let mut sink = [0u8; 64];
        while let Ok(n) = parent_pipe.read(&mut sink) {
            if n == 0 {
                break;
            }
        }
        unsafe {
            libc::kill(child_pid, libc::SIGKILL);
        }
    });

    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => {
            clear_signal_forwarder();
            return 1;
        }
    };
    clear_signal_forwarder();
    gpg_exit(status)
}

fn exec_native(real: &Path, argv0: &str, args: &[OsString]) -> i32 {
    let err = Command::new(real).arg0(argv0).args(args).exec();
    eprintln!("gpg: {err}");
    126
}

fn gpg_exit(status: ExitStatus) -> i32 {
    if let Some(sig) = status.signal() {
        let pid = std::process::id().to_string();
        let _ = Command::new("/bin/kill")
            .arg0("kill")
            .args([format!("-{sig}"), pid])
            .env_clear()
            .exec();
        return 128 + sig;
    }
    status.code().unwrap_or(1)
}

fn dump_option_table(executable: &Path) -> Result<HashMap<String, i32>, String> {
    let mut child = Command::new(executable)
        .args(["--no-options", "--dump-option-table"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("missing gpg stdout pipe".to_owned());
    };

    let reader_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let result = stdout
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_to_end(&mut buf);
        result.map(|_| buf)
    });

    let deadline = Instant::now() + OPTION_TABLE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("GPG option table timed out".to_owned());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err.to_string());
            }
        }
    };

    let data = reader_handle
        .join()
        .map_err(|_| "reader thread panicked".to_owned())?
        .map_err(|err| err.to_string())?;
    if !status.success() {
        return Err("GPG option table exited non-zero".to_owned());
    }
    if data.len() > MAX_MESSAGE_BYTES {
        return Err("GPG option table exceeds limit".to_owned());
    }
    let text = String::from_utf8_lossy(&data);
    gpg::parse_option_table(&text)
}

fn is_open_inheritable_fd(fd: RawFd) -> bool {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    flags >= 0 && (flags & libc::FD_CLOEXEC) == 0
}

fn make_pipe() -> std::io::Result<(File, File)> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `pipe` succeeded and returned two newly allocated open FDs.
    let read_file = unsafe { File::from_raw_fd(fds[0]) };
    let write_file = unsafe { File::from_raw_fd(fds[1]) };
    set_cloexec(read_file.as_raw_fd())?;
    set_cloexec(write_file.as_raw_fd())?;
    Ok((read_file, write_file))
}

fn set_cloexec(fd: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn clear_cloexec(fd: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
