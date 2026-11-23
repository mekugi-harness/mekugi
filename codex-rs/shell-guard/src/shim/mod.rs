mod client;
mod gpg;
mod paths;
mod shell;
mod sudo;
mod vcs;

use crate::protocol::ARG1_GPG_COMMAND;
use crate::protocol::ARG1_GPG_COMMAND_DEFAULT;
use crate::protocol::ARG1_GPG_RUN;
use crate::protocol::ARG1_SHELL;
use crate::protocol::ARG1_SHELL_DEFAULT;
use crate::protocol::ARG1_SUDO_COMMAND;
use crate::protocol::ARG1_SUDO_COMMAND_DEFAULT;
use crate::protocol::ARG1_VCS_COMMAND;
use crate::protocol::ARG1_VCS_COMMAND_DEFAULT;
use crate::protocol::ASKPASS_NAME;
use crate::protocol::ITEM_ID_ENV_VAR;
use crate::protocol::SOCKET_ENV_VAR;
use crate::protocol::SUDO_DIR_ENV_VAR;
use crate::protocol::THREAD_ID_ENV_VAR;
use crate::protocol::VCS_DIR_ENV_VAR;
use crate::protocol::VCS_TOOLS;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

/// Common invocation context parsed from explicit rewriter flags or inherited env.
#[derive(Debug, Clone)]
pub(crate) struct ShimContext {
    pub socket_path: Option<PathBuf>,
    pub vcs_dir: Option<PathBuf>,
    pub sudo_dir: Option<PathBuf>,
    pub thread_id: String,
    pub item_id: String,
}

impl ShimContext {
    pub(crate) fn from_env() -> Self {
        Self {
            socket_path: std::env::var_os(SOCKET_ENV_VAR)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            vcs_dir: std::env::var_os(VCS_DIR_ENV_VAR)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            sudo_dir: std::env::var_os(SUDO_DIR_ENV_VAR)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            thread_id: std::env::var(THREAD_ID_ENV_VAR).unwrap_or_default(),
            item_id: std::env::var(ITEM_ID_ENV_VAR).unwrap_or_default(),
        }
    }
}

/// Checks whether the current process was invoked as a shell-guard helper (via explicit
/// `argv[1]` flags from the AST rewriter / GPG supervisor, or via a `PATH` shim symlink).
///
/// Returns `Some(exit_code)` if dispatched, or `None` if this is a normal `mekugi-app-server`
/// invocation. Must be called from `codex-arg0` before spawning threads or Tokio.
pub fn try_dispatch_arg0() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().collect();
    let argv0 = args.first()?.to_string_lossy().into_owned();
    let argv1 = args
        .get(1)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Check explicit rewriter/supervisor flags BEFORE argv[0] basename so `exec -a`
    // can give the target any argv[0] without hijacking dispatch.
    match argv1.as_str() {
        ARG1_GPG_RUN => return Some(gpg::run_gpg_supervisor(&args[2..])),
        ARG1_VCS_COMMAND | ARG1_VCS_COMMAND_DEFAULT => {
            let default_path = argv1 == ARG1_VCS_COMMAND_DEFAULT;
            return Some(dispatch_explicit(
                &argv0,
                &args[2..],
                ExplicitMode::Vcs { default_path },
            ));
        }
        ARG1_SUDO_COMMAND | ARG1_SUDO_COMMAND_DEFAULT => {
            let default_path = argv1 == ARG1_SUDO_COMMAND_DEFAULT;
            return Some(dispatch_explicit(
                &argv0,
                &args[2..],
                ExplicitMode::Sudo { default_path },
            ));
        }
        ARG1_GPG_COMMAND | ARG1_GPG_COMMAND_DEFAULT => {
            let default_path = argv1 == ARG1_GPG_COMMAND_DEFAULT;
            return Some(dispatch_explicit(
                &argv0,
                &args[2..],
                ExplicitMode::Gpg { default_path },
            ));
        }
        ARG1_SHELL | ARG1_SHELL_DEFAULT => {
            let default_path = argv1 == ARG1_SHELL_DEFAULT;
            return Some(dispatch_explicit(
                &argv0,
                &args[2..],
                ExplicitMode::Shell { default_path },
            ));
        }
        _ => {}
    }

    let exe_name = Path::new(&argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let rest = &args[1..];

    if exe_name == ASKPASS_NAME {
        return Some(sudo::run_askpass());
    }
    if exe_name == "sudo" {
        return Some(sudo::run_sudo(
            &ShimContext::from_env(),
            "sudo",
            &argv0,
            /*default_path*/ false,
            rest,
        ));
    }
    if exe_name == "gpg" || exe_name == "gpg2" {
        return Some(gpg::run_gpg(
            &ShimContext::from_env(),
            exe_name,
            &argv0,
            /*default_path*/ false,
            rest,
        ));
    }
    if VCS_TOOLS.contains(&exe_name) {
        return Some(vcs::run_vcs(
            &ShimContext::from_env(),
            exe_name,
            &argv0,
            /*default_path*/ false,
            rest,
        ));
    }

    None
}

enum ExplicitMode {
    Vcs { default_path: bool },
    Sudo { default_path: bool },
    Gpg { default_path: bool },
    Shell { default_path: bool },
}

fn dispatch_explicit(argv0: &str, raw_args: &[OsString], mode: ExplicitMode) -> i32 {
    let mut ctx = ShimContext::from_env();
    let mut i = 0;
    while i < raw_args.len() {
        let flag = raw_args[i].to_string_lossy();
        match flag.as_ref() {
            "--" => {
                i += 1;
                break;
            }
            "--socket" if i + 1 < raw_args.len() => {
                ctx.socket_path = Some(PathBuf::from(&raw_args[i + 1]));
                i += 2;
            }
            "--vcs-dir" if i + 1 < raw_args.len() => {
                ctx.vcs_dir = Some(PathBuf::from(&raw_args[i + 1]));
                i += 2;
            }
            "--sudo-dir" if i + 1 < raw_args.len() => {
                ctx.sudo_dir = Some(PathBuf::from(&raw_args[i + 1]));
                i += 2;
            }
            "--thread" if i + 1 < raw_args.len() => {
                ctx.thread_id = raw_args[i + 1].to_string_lossy().into_owned();
                i += 2;
            }
            "--item" if i + 1 < raw_args.len() => {
                ctx.item_id = raw_args[i + 1].to_string_lossy().into_owned();
                i += 2;
            }
            _ => break,
        }
    }

    let Some(target_os) = raw_args.get(i) else {
        eprintln!("mekugi: missing guarded target command");
        return 1;
    };
    let Some(target) = target_os.to_str() else {
        eprintln!("mekugi: cannot identify non-UTF-8 executable");
        return 1;
    };
    let target_args = &raw_args[i + 1..];
    let forwarded = paths::forwarded_argv0(argv0, target);

    match mode {
        ExplicitMode::Vcs { default_path } => {
            vcs::run_vcs(&ctx, target, &forwarded, default_path, target_args)
        }
        ExplicitMode::Sudo { default_path } => {
            sudo::run_sudo(&ctx, target, &forwarded, default_path, target_args)
        }
        ExplicitMode::Gpg { default_path } => {
            gpg::run_gpg(&ctx, target, &forwarded, default_path, target_args)
        }
        ExplicitMode::Shell { default_path } => {
            shell::run_shell(&ctx, target, &forwarded, default_path, target_args)
        }
    }
}
