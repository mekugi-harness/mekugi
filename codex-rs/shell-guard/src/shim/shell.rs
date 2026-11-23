use crate::protocol::SHELLS;
use crate::rewriter::RewriteConfig;
use crate::rewriter::rewrite_script;
use crate::rewriter::shell_payload;
use crate::shim::ShimContext;
use crate::shim::paths::guard_environment;
use crate::shim::paths::guard_paths;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

/// Nested `-c` shell trampoline. Runs after the parent shell expands arguments,
/// lazily rewrites any `-c` payload with the minimal AST rewriter, and execs the
/// requested shell preserving positional parameters, flags, and environment.
pub(crate) fn run_shell(
    ctx: &ShimContext,
    target: &str,
    argv0: &str,
    default_path: bool,
    args: &[std::ffi::OsString],
) -> i32 {
    let base = Path::new(target)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(target);
    if !SHELLS.contains(&base) {
        return 1;
    }
    let Ok(helper) = std::env::current_exe() else {
        eprintln!("mekugi: cannot resolve shell guard helper");
        return 1;
    };
    let explicit = target.contains('/').then_some(target);
    let (_guard_dir, real) = match guard_paths(base, explicit, default_path) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("{err}");
            return 127;
        }
    };

    let mut forwarded_args = args.to_vec();
    if let Some(idx) = shell_payload(args)
        && let Some(socket_path) = ctx.socket_path.as_deref()
    {
        // If guard dirs are not currently on PATH (e.g., inside `env -i sh -c '...'`),
        // force rewriting bare guarded commands inside the nested payload without
        // mutating the clean child environment.
        let force_rewrite_bare =
            !path_has_guard_dirs(ctx.vcs_dir.as_deref(), ctx.sudo_dir.as_deref());
        let rewrite_cfg = RewriteConfig {
            helper_exe: &helper,
            socket_path,
            vcs_dir: ctx.vcs_dir.as_deref(),
            sudo_dir: ctx.sudo_dir.as_deref(),
            thread_id: &ctx.thread_id,
            item_id: &ctx.item_id,
            force_rewrite_bare,
        };
        let Some(script) = forwarded_args[idx].to_str() else {
            eprintln!("mekugi: cannot instrument non-UTF-8 shell source");
            return 1;
        };
        forwarded_args[idx] = rewrite_script(script, &rewrite_cfg).into();
    }

    let environment = guard_environment(&real);
    let mut cmd = Command::new(&real);
    cmd.arg0(argv0)
        .args(&forwarded_args)
        .env_clear()
        .envs(environment);
    let err = cmd.exec();
    eprintln!("{err}");
    126
}

fn path_has_guard_dirs(vcs_dir: Option<&Path>, sudo_dir: Option<&Path>) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    let entries: Vec<_> = std::env::split_paths(&path_var).collect();
    let has_vcs = vcs_dir.is_none_or(|dir| entries.iter().any(|e| e == dir));
    let has_sudo = sudo_dir.is_none_or(|dir| entries.iter().any(|e| e == dir));
    has_vcs && has_sudo
}
