use crate::protocol::ARG1_GPG_COMMAND;
use crate::protocol::ARG1_SHELL;
use crate::protocol::ARG1_SUDO_COMMAND;
use crate::protocol::ARG1_VCS_COMMAND;
use crate::protocol::ARG1_VCS_COMMAND_DEFAULT;
use crate::rewriter::RewriteConfig;
use crate::rewriter::rewrite_script;
use pretty_assertions::assert_eq;
use std::path::Path;

fn cfg(force_rewrite_bare: bool) -> RewriteConfig<'static> {
    RewriteConfig {
        helper_exe: Path::new("/opt/mekugi-app-server"),
        socket_path: Path::new("/tmp/codex-sg/s.sock"),
        vcs_dir: Some(Path::new("/tmp/arg0/vcs-guard")),
        sudo_dir: Some(Path::new("/tmp/arg0/sudo-guard")),
        thread_id: "thr_1",
        item_id: "call_1",
        force_rewrite_bare,
    }
}

#[test]
fn leaves_bare_commands_untouched_for_path_shims() {
    let config = cfg(false);
    assert_eq!(
        rewrite_script("git push origin main", &config),
        "git push origin main"
    );
    assert_eq!(rewrite_script("sudo id", &config), "sudo id");
    assert_eq!(
        rewrite_script("gpg --detach-sign a.txt", &config),
        "gpg --detach-sign a.txt"
    );
    assert_eq!(
        rewrite_script("timeout 10 git push", &config),
        "timeout 10 git push"
    );
    assert_eq!(
        rewrite_script("echo /usr/bin/git push", &config),
        "echo /usr/bin/git push"
    );
    assert_eq!(
        rewrite_script("command -v /usr/bin/git", &config),
        "command -v /usr/bin/git"
    );
}

#[test]
fn rewrites_explicit_paths_and_path_bypass_wrappers() {
    let config = cfg(false);

    for script in [
        "sh",
        "sh task.sh -c ignored",
        "bash --rcfile config script.sh",
    ] {
        assert_eq!(rewrite_script(script, &config), script);
    }

    let abs_git = rewrite_script("/usr/bin/git push origin main", &config);
    assert!(abs_git.contains(ARG1_VCS_COMMAND), "got: {abs_git}");
    assert!(
        abs_git.ends_with("-- /usr/bin/git push origin main"),
        "got: {abs_git}"
    );

    let abs_sudo = rewrite_script("/usr/bin/sudo -u root id", &config);
    assert!(abs_sudo.contains(ARG1_SUDO_COMMAND), "got: {abs_sudo}");
    assert!(abs_sudo.contains("--sudo-dir"), "got: {abs_sudo}");
    assert!(
        abs_sudo.ends_with("-- /usr/bin/sudo -u root id"),
        "got: {abs_sudo}"
    );

    let abs_gpg = rewrite_script("/usr/local/bin/gpg2 --sign", &config);
    assert!(abs_gpg.contains(ARG1_GPG_COMMAND), "got: {abs_gpg}");
    assert!(
        abs_gpg.ends_with("-- /usr/local/bin/gpg2 --sign"),
        "got: {abs_gpg}"
    );

    let env_i = rewrite_script("env -i git push", &config);
    assert!(
        env_i.starts_with("env -i '/opt/mekugi-app-server'"),
        "got: {env_i}"
    );
    assert!(env_i.contains(ARG1_VCS_COMMAND), "got: {env_i}");
    assert!(env_i.ends_with("-- git push"), "got: {env_i}");

    let cmd_p = rewrite_script("command -p git push", &config);
    assert!(cmd_p.contains(ARG1_VCS_COMMAND_DEFAULT), "got: {cmd_p}");
    assert!(cmd_p.ends_with("-- git push"), "got: {cmd_p}");

    let timeout_abs = rewrite_script("timeout 10 /usr/bin/git push", &config);
    assert!(
        timeout_abs.starts_with("timeout 10 '/opt/mekugi-app-server'"),
        "got: {timeout_abs}"
    );

    let dynamic_suffix = rewrite_script("\"$tools/git\" push", &config);
    assert!(
        dynamic_suffix.contains(ARG1_VCS_COMMAND),
        "got: {dynamic_suffix}"
    );
    assert!(
        dynamic_suffix.ends_with("-- \"$tools/git\" push"),
        "got: {dynamic_suffix}"
    );

    let local_path = rewrite_script("PATH=/usr/bin git push", &config);
    assert!(
        local_path.starts_with("PATH=/usr/bin '/opt/mekugi-app-server'"),
        "got: {local_path}"
    );

    let script_path = rewrite_script("PATH=/usr/bin; git push", &config);
    assert!(script_path.contains(ARG1_VCS_COMMAND), "got: {script_path}");

    let nested_sh = rewrite_script("sh -c '/usr/bin/git push'", &config);
    assert!(nested_sh.contains(ARG1_SHELL), "got: {nested_sh}");
    assert!(
        nested_sh.ends_with("-- sh -c '/usr/bin/git push'"),
        "got: {nested_sh}"
    );

    let unreached = rewrite_script("false && /usr/bin/git push", &config);
    assert!(
        unreached.starts_with("false && '/opt/mekugi-app-server'"),
        "got: {unreached}"
    );

    let escaped = rewrite_script("/usr/bin/g\\it push", &config);
    assert!(escaped.contains(ARG1_VCS_COMMAND), "got: {escaped}");
}

#[test]
fn force_rewrite_bare_rewrites_bare_commands_for_cleared_env_shells() {
    let config = cfg(true);
    let rewritten = rewrite_script("git push && sudo id", &config);
    assert!(rewritten.contains(ARG1_VCS_COMMAND), "got: {rewritten}");
    assert!(rewritten.contains(ARG1_SUDO_COMMAND), "got: {rewritten}");
}
