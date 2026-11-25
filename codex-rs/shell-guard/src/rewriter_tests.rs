use crate::protocol::ARG1_GPG_COMMAND;
use crate::protocol::ARG1_SHELL;
use crate::protocol::ARG1_SUDO_COMMAND;
use crate::protocol::ARG1_VCS_COMMAND;
use crate::protocol::ARG1_VCS_COMMAND_DEFAULT;
use crate::rewriter::RewriteConfig;
use crate::rewriter::command_word_nodes;
use crate::rewriter::parse_bash;
use crate::rewriter::rewrite_script;
use crate::rewriter::rewrite_script_tree;
use crate::rewriter::static_word_value;
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

#[test]
fn parse_bash_and_rewrite_script_tree_are_reusable() {
    let config = cfg(false);
    let script = "FOO=1 /usr/bin/git 'push' \"origin\" main";
    let tree = parse_bash(script).expect("valid bash script");

    assert_eq!(
        rewrite_script_tree(script, &tree, &config),
        rewrite_script(script, &config)
    );

    let command = tree.root_node().named_child(0).expect("top-level command");
    let words: Vec<Option<String>> = command_word_nodes(command)
        .into_iter()
        .map(|node| static_word_value(node, script))
        .collect();
    assert_eq!(
        words,
        vec![
            Some("/usr/bin/git".to_owned()),
            Some("push".to_owned()),
            Some("origin".to_owned()),
            Some("main".to_owned()),
        ]
    );
}

#[test]
fn leaves_quoted_heredoc_bodies_unchanged_across_operators_and_delimiters() {
    let config = cfg(false);
    let cases = [
        "cat <<'EOF' > /tmp/remote.sh &&\n#!/bin/sh\nsh -c 'echo hi'\n/usr/bin/git status\nEOF\necho ok",
        "cat <<'EOF' > /tmp/remote.sh ||\nsh -c 'echo hi'\nEOF\necho ok",
        "cat <<'EOF' |\nsh -c 'echo hi'\nEOF\ngrep hi",
        "cat <<'EOF' |&\nsh -c 'echo hi'\nEOF\ngrep hi",
        "cat <<'EOF'; echo ok\nsh -c 'echo hi'\nEOF",
        "cat <<-'EOF' > /tmp/remote.sh &&\n\t#!/bin/sh\n\tsh -c 'echo hi'\n\tEOF\necho ok",
        "cat <<\"EOF\" > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\necho ok",
        "cat <<\\EOF > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\necho ok",
        "cat <<E'O'F > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\necho ok",
        "cat <<$'EOF' > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\necho ok",
        "{ cat <<'A'; cat <<'B'; } &&\nsh -c 'echo a'\nA\n/usr/bin/git status\nB\necho ok",
        "{ cat <<UNQ; cat <<'QUOTED'; } &&\nplain text\nUNQ\nsh -c 'echo hi'\nQUOTED\necho ok",
        "out=\"$(cat <<'EOF' &&\nsh -c 'echo hi'\nEOF\necho ok)\"",
    ];

    for script in cases {
        assert_eq!(rewrite_script(script, &config), script, "script: {script}");
        let tree = parse_bash(script).expect("parse script");
        assert_eq!(
            rewrite_script_tree(script, &tree, &config),
            script,
            "rewrite_script_tree failed for: {script}"
        );
    }

    let forced = cfg(true);
    let forced_script =
        "cat <<'EOF' > /tmp/remote.sh &&\ngit push origin main\nsudo id\nEOF\ngit status";
    let rewritten = rewrite_script(forced_script, &forced);
    assert!(
        rewritten
            .starts_with("cat <<'EOF' > /tmp/remote.sh &&\ngit push origin main\nsudo id\nEOF\n"),
        "heredoc body must remain verbatim, got: {rewritten}"
    );
    assert!(
        rewritten.ends_with("-- git status"),
        "command after heredoc must still be rewritten, got: {rewritten}"
    );
}

#[test]
fn quoted_heredoc_does_not_mutate_path_or_block_surrounding_rewrites() {
    let config = cfg(false);

    // PATH mutations inside a quoted heredoc must not mark the outer script as mutating PATH.
    let path_in_heredoc = "cat <<'EOF' > /tmp/remote.sh &&\nPATH=/usr/bin\nexport PATH=/bin\nunset PATH\nEOF\ngit push origin main";
    assert_eq!(rewrite_script(path_in_heredoc, &config), path_in_heredoc);

    // Unbalanced quotes inside a quoted heredoc must not derail rewriting of commands after it.
    let unbalanced_body = "cat <<'EOF' > /tmp/remote.sh &&\n#!/bin/sh\necho \"don't break\nsh -c 'echo hi'\nEOF\n/usr/bin/git status";
    let rewritten_after = rewrite_script(unbalanced_body, &config);
    assert!(
        rewritten_after.starts_with(
            "cat <<'EOF' > /tmp/remote.sh &&\n#!/bin/sh\necho \"don't break\nsh -c 'echo hi'\nEOF\n"
        ),
        "got: {rewritten_after}"
    );
    assert!(
        rewritten_after.contains(ARG1_VCS_COMMAND)
            && rewritten_after.ends_with("-- /usr/bin/git status"),
        "got: {rewritten_after}"
    );

    // Guarded commands on the same line as a quoted heredoc must still be rewritten.
    let same_line_cmd = "cat <<'EOF' && /usr/bin/git status\nsh -c 'echo hi'\nEOF";
    let rewritten_same_line = rewrite_script(same_line_cmd, &config);
    assert!(
        rewritten_same_line.contains(ARG1_VCS_COMMAND)
            && rewritten_same_line.ends_with("-- /usr/bin/git status\nsh -c 'echo hi'\nEOF"),
        "got: {rewritten_same_line}"
    );

    // Explicit FD prefix and line continuation before a quoted heredoc body.
    let fd_prefixed = "3<<'EOF' /usr/bin/git status\nsh -c 'echo hi'\nEOF";
    let rewritten_fd = rewrite_script(fd_prefixed, &config);
    assert!(
        rewritten_fd.contains(ARG1_VCS_COMMAND)
            && rewritten_fd.ends_with("-- /usr/bin/git status\nsh -c 'echo hi'\nEOF"),
        "got: {rewritten_fd}"
    );
    let continued =
        "cat <<'EOF' \\\n  > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\n/usr/bin/git status";
    let rewritten_continued = rewrite_script(continued, &config);
    assert!(
        rewritten_continued
            .starts_with("cat <<'EOF' \\\n  > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\n")
            && rewritten_continued.ends_with("-- /usr/bin/git status"),
        "got: {rewritten_continued}"
    );

    // Comments, strings, here-strings, and arithmetic shifts must not be treated as quoted heredocs.
    for non_heredoc in [
        "# cat <<'EOF' &&\nsh -c 'echo real'",
        "echo \"cat <<'EOF' &&\"\nsh -c 'echo real'",
        "echo '<<EOF'\nsh -c 'echo real'",
        "cat <<< 'EOF' &&\nsh -c 'echo real'",
        "x=$((1 << 2))\nsh -c 'echo real'",
    ] {
        let rewritten = rewrite_script(non_heredoc, &config);
        assert!(
            rewritten.contains(ARG1_SHELL) && rewritten.ends_with("-- sh -c 'echo real'"),
            "non-heredoc was not rewritten ({non_heredoc}): {rewritten}"
        );
    }

    // Unquoted heredocs still rewrite command substitutions executed by the generating shell.
    let unquoted = "cat <<EOF\n$(/usr/bin/git status)\nEOF";
    let rewritten_unquoted = rewrite_script(unquoted, &config);
    assert!(
        rewritten_unquoted.contains(ARG1_VCS_COMMAND),
        "got: {rewritten_unquoted}"
    );
}

#[test]
fn prevents_heredoc_bypass_and_continuation_argument_rewrites() {
    let config = cfg(false);

    // Backslash-newline inside a quoted heredoc redirection must keep following args as args.
    let continued_arg = "printf '%s\\n' <<\\\n'EOF' /usr/bin/git\ncontent\nEOF";
    assert_eq!(rewrite_script(continued_arg, &config), continued_arg);

    // Arithmetic shift inside (( ... )) must not be mistaken for a quoted heredoc.
    let arith_shift = "((1 << \"2\"))\n/usr/bin/git push";
    let rewritten_arith = rewrite_script(arith_shift, &config);
    assert!(
        rewritten_arith.contains(ARG1_VCS_COMMAND)
            && rewritten_arith.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_arith}"
    );

    // Parameter expansion containing <<'END' must not be mistaken for a quoted heredoc.
    let param_exp = "echo ${foo:-<<'END'}\n/usr/bin/git push";
    let rewritten_param = rewrite_script(param_exp, &config);
    assert!(
        rewritten_param.contains(ARG1_VCS_COMMAND)
            && rewritten_param.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_param}"
    );

    // Literal <<'END' inside an unquoted heredoc body must be skipped, not parsed as a heredoc.
    let unquoted_fake = "cat <<UNQ\n<<'END'\nUNQ\n/usr/bin/git push";
    let rewritten_fake = rewrite_script(unquoted_fake, &config);
    assert!(
        rewritten_fake.contains(ARG1_VCS_COMMAND)
            && rewritten_fake.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_fake}"
    );

    // Nested unquoted heredoc inside $(...) after an outer quoted redirect keeps separate queues.
    let nested_queues = "cat <<'OUTER' $(cat <<INNER\n$(/usr/bin/git push)\nINNER\n)\nOUTER";
    let rewritten_nested = rewrite_script(nested_queues, &config);
    assert!(
        rewritten_nested.contains(ARG1_VCS_COMMAND),
        "got: {rewritten_nested}"
    );

    // ANSI-C hex/octal/unicode escapes in heredoc delimiters must match Bash's decoded delimiter.
    for ansi_c in [
        "cat <<$'\\x45OF'\nhello\nEOF\n/usr/bin/git push",
        "cat <<$'\\105OF'\nhello\nEOF\n/usr/bin/git push",
        "cat <<$'\\u0045OF'\nhello\nEOF\n/usr/bin/git push",
    ] {
        let rewritten = rewrite_script(ansi_c, &config);
        assert!(
            rewritten.contains(ARG1_VCS_COMMAND) && rewritten.ends_with("-- /usr/bin/git push"),
            "ANSI-C delimiter failed ({ansi_c}): {rewritten}"
        );
    }

    // ANSI-C control escape before a multibyte UTF-8 character must not panic and truncates at NUL.
    let unicode_ctrl = "cat <<$'EOF\\c\u{0800}' &&\nsh -c 'echo hi'\nEOF\n/usr/bin/git push";
    let rewritten_unicode = rewrite_script(unicode_ctrl, &config);
    assert!(
        rewritten_unicode.starts_with("cat <<$'EOF\\c\u{0800}' &&\nsh -c 'echo hi'\nEOF\n")
            && rewritten_unicode.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_unicode}"
    );

    // Literal brace inside parameter expansion and nested parameter expansion must not hide later heredocs.
    for brace_case in [
        "printf '%s\\n' \"${value:-{text}\"\ncat <<'EOF' &&\nsh -c 'echo hi'\nEOF\necho ok",
        "printf '%s\\n' \"${a:-${b:-text}}\"\ncat <<'EOF' &&\nsh -c 'echo hi'\nEOF\necho ok",
    ] {
        assert_eq!(rewrite_script(brace_case, &config), brace_case);
    }

    // Unquoted heredoc followed by a quoted heredoc must protect the quoted body and still rewrite later commands.
    let unquoted_then_quoted =
        "cat <<EOF\nhello\nEOF\ncat <<'Q' &&\nsh -c 'echo hi'\nQ\n/usr/bin/git push";
    let rewritten_seq = rewrite_script(unquoted_then_quoted, &config);
    assert!(
        rewritten_seq.starts_with("cat <<EOF\nhello\nEOF\ncat <<'Q' &&\nsh -c 'echo hi'\nQ\n")
            && rewritten_seq.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_seq}"
    );

    // Array subscript shift must not be parsed as a heredoc hiding later quoted heredocs.
    let array_shift = "a[1<<2]=x\ncat <<'EOF' &&\nsh -c 'echo hi'\nEOF\necho ok";
    assert_eq!(rewrite_script(array_shift, &config), array_shift);

    // Command substitution inside arithmetic command must still protect its own quoted heredoc.
    let arith_cmd_subst =
        "(( result = $(cat <<'EOF' > /tmp/remote.sh &&\nsh -c 'echo hi'\nEOF\necho 1) ))";
    assert_eq!(rewrite_script(arith_cmd_subst, &config), arith_cmd_subst);

    // Quoted heredoc nested inside command substitution within an unquoted heredoc body must stay protected.
    let unquoted_nested_quoted =
        "cat <<EOF\n$(cat <<'INNER' > /tmp/remote.sh &&\nsh -c 'echo hi'\nINNER\necho ok)\nEOF";
    assert_eq!(
        rewrite_script(unquoted_nested_quoted, &config),
        unquoted_nested_quoted
    );

    // Case pattern inside command substitution must not pop CmdSubst and drop a queued heredoc.
    let case_in_subst = "out=$(cat <<'EOF'; case x in a) echo ok;; esac\nsh -c 'echo hi'\nEOF\n)";
    assert_eq!(rewrite_script(case_in_subst, &config), case_in_subst);

    // Literal case/in arguments inside a command substitution must not strand an outer quoted heredoc.
    let case_in_args =
        "cat <<'EOF' > \"$(printf '%s-%s.sh' case in)\" &&\nsh -c 'echo hi'\nEOF\necho ok";
    assert_eq!(rewrite_script(case_in_args, &config), case_in_args);

    // Trailing comment backslash and escaped double-backslash must keep the heredoc body verbatim
    // while still rewriting guarded commands that follow the terminator.
    for (prefix, suffix) in [
        (
            "cat <<'SCRIPT' && # note \\\nsh -c 'echo hello'\nSCRIPT\n",
            "/usr/bin/git push",
        ),
        (
            "cat <<'SCRIPT' && echo \\\\\nsh -c 'echo hello'\nSCRIPT\n",
            "/usr/bin/git push",
        ),
    ] {
        let script = format!("{prefix}{suffix}");
        let rewritten = rewrite_script(&script, &config);
        assert!(
            rewritten.starts_with(prefix) && rewritten.ends_with("-- /usr/bin/git push"),
            "got: {rewritten}"
        );
    }

    // Quoted prefix concatenated with unquoted suffix ('END'MARKER) must not terminate early on END.
    let concat_delim = "cat <<'END'MARKER &&\nEND\nsh -c 'echo hello'\nENDMARKER\necho ok";
    assert_eq!(rewrite_script(concat_delim, &config), concat_delim);

    // Delimiter containing literal <(tag) suffix (E'O'F<(tag)) must protect the full body
    // while still rewriting commands after the real terminator.
    let subst_like_prefix = "cat <<E'O'F<(tag) &&\nEOF\nsh -c 'printf done'\nEOF<(tag)\n";
    let subst_like_script = format!("{subst_like_prefix}/usr/bin/git push");
    let rewritten_subst = rewrite_script(&subst_like_script, &config);
    assert!(
        rewritten_subst.starts_with(subst_like_prefix)
            && rewritten_subst.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_subst}"
    );

    // Unquoted delimiter adjacent to an output redirect (<<EOF>/dev/null) must not absorb >/dev/null.
    let adjacent_prefix = "cat <<EOF>/dev/null && cat <<'Q' > script.sh\nhello\nEOF\nvalue=$(sh -c 'printf sample')\nQ\n";
    let adjacent_script = format!("{adjacent_prefix}/usr/bin/git push");
    let rewritten_adjacent = rewrite_script(&adjacent_script, &config);
    assert!(
        rewritten_adjacent.starts_with(adjacent_prefix)
            && rewritten_adjacent.ends_with("-- /usr/bin/git push"),
        "got: {rewritten_adjacent}"
    );

    // Delimiters containing balanced parameter expansions or backticks with semicolons must keep
    // the trailing quoted fragment while still rewriting commands after the real terminator.
    for delim_prefix in [
        "cat <<EOF${tag:-a;b}'Z' &&\nsh -c 'printf done'\nEOF${tag:-a;b}Z\n",
        "cat <<EOF`a;b`'Z' &&\nsh -c 'printf done'\nEOF`a;b`Z\n",
    ] {
        let script = format!("{delim_prefix}/usr/bin/git push");
        let rewritten = rewrite_script(&script, &config);
        assert!(
            rewritten.starts_with(delim_prefix) && rewritten.ends_with("-- /usr/bin/git push"),
            "got: {rewritten}"
        );
    }

    // Multiline process substitution on a quoted heredoc line must still rewrite its inner command.
    let proc_subst = "cat <<'OUTER' <(\n/usr/bin/git push\n)\nOUTER";
    let rewritten_proc = rewrite_script(proc_subst, &config);
    assert!(
        rewritten_proc.contains(ARG1_VCS_COMMAND),
        "process substitution bypassed approval: {rewritten_proc}"
    );
}

#[cfg(unix)]
#[test]
fn generated_script_from_quoted_heredoc_executes_without_generating_host_helper() {
    use std::fs;
    use std::process::Command;

    let dir = tempfile::tempdir().expect("tempdir");
    let remote_script = dir.path().join("remote-validate.sh");
    let host_only_helper = Path::new("/nonexistent-generating-host/bin/mekugi-app-server");
    let config = RewriteConfig {
        helper_exe: host_only_helper,
        socket_path: Path::new("/nonexistent-generating-host/s.sock"),
        vcs_dir: Some(Path::new("/nonexistent-generating-host/vcs-guard")),
        sudo_dir: Some(Path::new("/nonexistent-generating-host/sudo-guard")),
        thread_id: "thr_gen",
        item_id: "call_gen",
        force_rewrite_bare: false,
    };

    let expected_body = "#!/bin/sh\nset -eu\nsh -c 'printf \"%s\\n\" \"remote-ok\"'\n";
    let generator_script = format!(
        "cat <<'EOF' > '{}' &&\n{expected_body}EOF\nchmod +x '{}'",
        remote_script.display(),
        remote_script.display()
    );

    let rewritten = rewrite_script(&generator_script, &config);
    assert_eq!(rewritten, generator_script);
    assert!(
        !rewritten.contains("/nonexistent-generating-host/"),
        "host-local helper leaked into rewritten script: {rewritten}"
    );

    // Step 1: run generator script on generating host.
    let gen_status = Command::new("/bin/sh")
        .arg("-c")
        .arg(&rewritten)
        .status()
        .expect("run generator script");
    assert!(
        gen_status.success(),
        "generator script failed: {gen_status}"
    );

    let written_body = fs::read_to_string(&remote_script).expect("read generated script");
    assert_eq!(written_body, expected_body);

    // Step 2: execute generated script on a target host where host_only_helper does not exist.
    assert!(!host_only_helper.exists());
    let exec_output = Command::new("/bin/sh")
        .arg(&remote_script)
        .output()
        .expect("run generated remote script");
    assert!(
        exec_output.status.success(),
        "remote script failed: status={} stderr={}",
        exec_output.status,
        String::from_utf8_lossy(&exec_output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&exec_output.stdout), "remote-ok\n");
}
