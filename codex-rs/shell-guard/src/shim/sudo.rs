use crate::classifier::sudo;
use crate::protocol::ASKPASS_NAME;
use crate::protocol::GuardKind;
use crate::protocol::MAX_MESSAGE_BYTES;
use crate::protocol::SOCKET_ENV_VAR;
use crate::protocol::SUDO_ARGV_DISPLAY_BUDGET;
use crate::protocol::SUDO_COMMAND_ENV_VAR;
use crate::protocol::ShimRequest;
use crate::shim::ShimContext;
use crate::shim::client::send_request;
use crate::shim::paths::guard_paths;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// Execs real `sudo` once after command approval (if applicable), injecting
/// `-A` and `SUDO_ASKPASS` for interactive password prompts.
pub(crate) fn run_sudo(
    ctx: &ShimContext,
    target: &str,
    argv0: &str,
    default_path: bool,
    args: &[OsString],
) -> i32 {
    let explicit = target.contains('/').then_some(target);
    let (found_dir, real) = match guard_paths("sudo", explicit, default_path) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("sudo: {err}");
            return 127;
        }
    };
    let sudo_dir = ctx.sudo_dir.clone().or(found_dir);

    let words: Vec<_> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if sudo::command_approval(&words) {
        if args.iter().any(|arg| arg.to_str().is_none()) {
            eprintln!("mekugi: cannot request exact approval for non-UTF-8 arguments");
            return 1;
        }
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(target.to_owned());
        argv.extend_from_slice(&words);
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let req = ShimRequest {
            kind: GuardKind::Sudo,
            thread_id: ctx.thread_id.clone(),
            item_id: ctx.item_id.clone(),
            cwd,
            argv,
            executable: real.to_string_lossy().into_owned(),
        };
        match send_request(ctx.socket_path.as_deref(), &req) {
            Ok(reply) if reply.ok => {}
            Ok(reply) => {
                let reason = reply.reason.as_deref().unwrap_or("denied");
                eprintln!("mekugi: sudo command denied: {reason}");
                return 1;
            }
            Err(reason) => {
                eprintln!("mekugi: sudo command denied: {reason}");
                return 1;
            }
        }
    }

    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let mut forwarded_args = args.to_vec();

    if sudo::interactive(&words) {
        let Some(sudo_dir) = sudo_dir else {
            eprintln!("mekugi: sudo askpass directory unavailable");
            return 1;
        };
        let Some(socket_path) = ctx.socket_path.as_deref() else {
            eprintln!("mekugi: sudo command denied: Mekugi approval is unavailable");
            return 1;
        };
        // Bound argv display metadata so no environment string approaches execve limits.
        let mut argv = vec![target.to_owned()];
        let mut budget = SUDO_ARGV_DISPLAY_BUDGET;
        for arg in &words {
            if arg.len() > budget {
                argv.push("[remaining sudo arguments omitted]".to_owned());
                break;
            }
            argv.push(arg.clone());
            budget = budget.saturating_sub(arg.len() + 1);
        }
        let message = ShimRequest {
            kind: GuardKind::SudoPassword,
            thread_id: ctx.thread_id.clone(),
            item_id: ctx.item_id.clone(),
            cwd: String::new(),
            argv,
            executable: real.to_string_lossy().into_owned(),
        };
        let Ok(encoded) = serde_json::to_string(&message) else {
            return 1;
        };
        if encoded.len() > MAX_MESSAGE_BYTES {
            return 1;
        }
        env.retain(|(k, _)| {
            k != OsStr::new("SUDO_ASKPASS")
                && k != OsStr::new(SUDO_COMMAND_ENV_VAR)
                && k != OsStr::new(SOCKET_ENV_VAR)
        });
        env.push((
            OsString::from("SUDO_ASKPASS"),
            sudo_dir.join(ASKPASS_NAME).into_os_string(),
        ));
        env.push((
            OsString::from(SUDO_COMMAND_ENV_VAR),
            OsString::from(encoded),
        ));
        env.push((
            OsString::from(SOCKET_ENV_VAR),
            socket_path.as_os_str().to_os_string(),
        ));
        forwarded_args.insert(0, "-A".into());
    }

    let mut cmd = Command::new(&real);
    cmd.arg0(argv0).args(&forwarded_args).env_clear().envs(env);
    let err = cmd.exec();
    eprintln!("sudo: {err}");
    126
}

/// Askpass helper (`mekugi-sudo-askpass`): writes the secret only to stdout (sudo's
/// askpass pipe) and zeroes the secret buffer on drop. Never writes secrets to stderr.
pub(crate) fn run_askpass() -> i32 {
    let Ok(raw_cmd) = std::env::var(SUDO_COMMAND_ENV_VAR) else {
        return 1;
    };
    let Ok(mut message) = serde_json::from_str::<ShimRequest>(&raw_cmd) else {
        return 1;
    };
    if message.kind != GuardKind::SudoPassword || message.argv.is_empty() {
        return 1;
    }
    message.cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let socket_path = std::env::var_os(SOCKET_ENV_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let Ok(reply) = send_request(socket_path.as_deref(), &message) else {
        return 1;
    };
    let Some(secret) = reply.secret.filter(|_| reply.ok) else {
        return 1;
    };
    if !sudo::valid_password(secret.as_str()) {
        return 1;
    }
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(secret.as_str().as_bytes()).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        return 1;
    }
    drop(secret);
    0
}
