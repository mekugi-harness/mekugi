use crate::classifier::remote;
use crate::protocol::GuardKind;
use crate::protocol::ShimRequest;
use crate::protocol::VCS_TOOLS;
use crate::shim::ShimContext;
use crate::shim::client::send_request;
use crate::shim::paths::guard_environment;
use crate::shim::paths::guard_paths;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

pub(crate) fn run_vcs(
    ctx: &ShimContext,
    target: &str,
    argv0: &str,
    default_path: bool,
    args: &[std::ffi::OsString],
) -> i32 {
    let name = Path::new(target)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(target);
    if !VCS_TOOLS.contains(&name) {
        return 1;
    }
    let explicit = target.contains('/').then_some(target);
    let (_guard_dir, real) = match guard_paths(name, explicit, default_path) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("{name}: {err}");
            return 127;
        }
    };

    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(name.to_owned());
    argv.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));

    let environment = guard_environment(&real);
    let lookup = |globals: &[String], alias: &str| -> Option<String> {
        // An alias query must use the same path bytes as the command.
        if args.iter().any(|arg| arg.to_str().is_none()) {
            return None;
        }
        // Strip write-observer interposition vars from the read-only git config probe
        // so DYLD_INSERT_LIBRARIES does not interfere with alias resolution on macOS.
        let mut cmd = Command::new(&real);
        cmd.args(globals)
            .args(["config", "--get", &format!("alias.{alias}")])
            .stdin(std::process::Stdio::null())
            .env_clear()
            .envs(
                environment
                    .iter()
                    .filter(|(k, _)| {
                        *k != std::ffi::OsStr::new("DYLD_INSERT_LIBRARIES")
                            && *k != std::ffi::OsStr::new("MEKUGI_WRITE_OBSERVER_SOCK")
                    })
                    .map(|(k, v)| (k, v)),
            );
        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Some(text.strip_suffix('\n').unwrap_or(&text).to_owned())
    };

    if remote::writes(&argv, Some(&lookup)) {
        if args.iter().any(|arg| arg.to_str().is_none()) {
            eprintln!("mekugi: cannot request exact approval for non-UTF-8 arguments");
            return 1;
        }
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let req = ShimRequest {
            kind: GuardKind::RemoteWrite,
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
                eprintln!("mekugi: remote write denied: {reason}");
                return 1;
            }
            Err(reason) => {
                eprintln!("mekugi: remote write denied: {reason}");
                return 1;
            }
        }
    }

    let mut cmd = Command::new(&real);
    cmd.arg0(argv0).args(args).env_clear().envs(environment);
    let err = cmd.exec();
    eprintln!("{name}: {err}");
    126
}
