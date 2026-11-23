use crate::classifier::gpg;
use crate::classifier::remote;
use crate::classifier::sudo;
use pretty_assertions::assert_eq;

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn git_remote_write_classification() {
    assert!(remote::writes(&s(&["git", "push"]), None));
    assert!(remote::writes(
        &s(&["/usr/bin/git", "-C", "/tmp", "push", "origin", "main"]),
        None
    ));
    assert!(!remote::writes(&s(&["git", "status"]), None));
    assert!(!remote::writes(&s(&["git", "commit", "-m", "push"]), None));
    assert!(!remote::writes(&s(&["git", "stash", "push"]), None));
    assert!(!remote::writes(&s(&["git", "push", "--dry-run"]), None));
    assert!(!remote::writes(&s(&["git", "push", "-n"]), None));
    assert!(remote::writes(
        &s(&["git", "push", "--dry-run", "--no-dry-run"]),
        None
    ));
    assert!(remote::writes(
        &s(&["git", "push", "-n", "--no-dry-ru"]),
        None
    ));
    assert!(!remote::writes(&s(&["git", "push", "--help"]), None));
    assert!(!remote::writes(&s(&["git", "push", "-h"]), None));
    assert!(remote::writes(&s(&["git", "push", "-o", "h"]), None));
    assert!(remote::writes(&s(&["git", "push", "--repo", "-n"]), None));
    assert!(remote::writes(
        &s(&["git", "send-email", "0001.patch"]),
        None
    ));
    assert!(!remote::writes(
        &s(&["git", "send-email", "--dry-run", "0001.patch"]),
        None
    ));
    assert!(remote::writes(
        &s(&["git", "subtree", "push", "--prefix=foo", "origin", "main"]),
        None
    ));
    assert!(remote::writes(&s(&["git", "svn", "dcommit"]), None));
    assert!(!remote::writes(&s(&["git", "svn", "rebase"]), None));
    assert!(remote::writes(&s(&["git", "p4", "submit"]), None));
    assert!(remote::writes(
        &s(&["git", "lfs", "push", "origin", "main"]),
        None
    ));
    assert!(remote::writes(
        &s(&["git", "submodule", "foreach", "--recursive", "git push"]),
        None
    ));
    assert!(!remote::writes(
        &s(&["git", "submodule", "foreach", "git status"]),
        None
    ));
    assert!(remote::writes(
        &s(&["git", "rebase", "-x", "git push", "HEAD~1"]),
        None
    ));
    assert!(remote::writes(
        &s(&["git", "bisect", "run", "git", "push"]),
        None
    ));
    // Unknown subcommand with no lookup fails closed.
    assert!(remote::writes(&s(&["git", "custom-cmd"]), None));
}

#[test]
fn git_alias_resolution_and_depth_limit() {
    let lookup = |globals: &[String], name: &str| -> Option<String> {
        match name {
            "p" => Some("push --force-with-lease".to_owned()),
            "st" => Some("status -sb".to_owned()),
            "shpush" => Some("!git commit -m ok && git push".to_owned()),
            "shsafe" => Some("!git status && echo ok".to_owned()),
            "loop" => Some("loop".to_owned()),
            "ctx" => {
                if globals.iter().any(|g| g == "/repo") {
                    Some("push".to_owned())
                } else {
                    Some("status".to_owned())
                }
            }
            _ => None,
        }
    };

    assert!(remote::writes(&s(&["git", "p"]), Some(&lookup)));
    assert!(!remote::writes(
        &s(&["git", "p", "--dry-run"]),
        Some(&lookup)
    ));
    assert!(!remote::writes(&s(&["git", "st"]), Some(&lookup)));
    assert!(remote::writes(&s(&["git", "shpush"]), Some(&lookup)));
    assert!(!remote::writes(&s(&["git", "shsafe"]), Some(&lookup)));
    // Recursive alias exceeds depth 8 -> fails closed.
    assert!(remote::writes(&s(&["git", "loop"]), Some(&lookup)));
    // Global flags passed through to alias lookup.
    assert!(remote::writes(
        &s(&["git", "-C", "/repo", "ctx"]),
        Some(&lookup)
    ));
    assert!(!remote::writes(&s(&["git", "ctx"]), Some(&lookup)));
    // Unknown external subcommand (lookup returns None) fails closed.
    assert!(remote::writes(&s(&["git", "unknown-ext"]), Some(&lookup)));
}

#[test]
fn gh_hg_svn_jj_classification() {
    // gh
    assert!(!remote::writes(&s(&["gh", "pr", "view", "123"]), None));
    assert!(remote::writes(&s(&["gh", "pr", "create"]), None));
    assert!(!remote::writes(&s(&["gh", "repo", "clone", "a/b"]), None));
    assert!(remote::writes(&s(&["gh", "label", "clone", "a/b"]), None));
    assert!(!remote::writes(
        &s(&["gh", "repo", "deploy-key", "list"]),
        None
    ));
    assert!(remote::writes(
        &s(&["gh", "repo", "deploy-key", "add", "key.pub"]),
        None
    ));
    assert!(!remote::writes(&s(&["gh", "api", "repos/a/b"]), None));
    assert!(remote::writes(
        &s(&["gh", "api", "-X", "POST", "repos/a/b/issues"]),
        None
    ));
    assert!(remote::writes(
        &s(&["gh", "api", "-f", "title=hi", "repos/a/b/issues"]),
        None
    ));
    assert!(!remote::writes(
        &s(&["gh", "api", "graphql", "-f", "query={ viewer { login } }"]),
        None
    ));
    assert!(remote::writes(
        &s(&["gh", "api", "graphql", "-f", "query=mutation { close }"]),
        None
    ));
    assert!(remote::writes(
        &s(&["gh", "api", "graphql", "-F", "query=@query.graphql"]),
        None
    ));

    // hg
    assert!(remote::writes(&s(&["hg", "push"]), None));
    assert!(remote::writes(&s(&["hg", "pus"]), None));
    assert!(!remote::writes(&s(&["hg", "pull"]), None));
    assert!(!remote::writes(&s(&["hg", "push", "--help"]), None));

    // svn
    assert!(remote::writes(&s(&["svn", "commit", "-m", "x"]), None));
    assert!(remote::writes(&s(&["svn", "ci"]), None));
    assert!(!remote::writes(&s(&["svn", "copy", "a", "b"]), None));
    assert!(remote::writes(
        &s(&["svn", "copy", "a", "https://example.com/svn/b"]),
        None
    ));
    assert!(remote::writes(&s(&["svn", "mkdir", "^/tags/v1"]), None));

    // jj
    assert!(remote::writes(&s(&["jj", "git", "push"]), None));
    assert!(!remote::writes(
        &s(&["jj", "git", "push", "--dry-run"]),
        None
    ));
    assert!(remote::writes(&s(&["jj", "gerrit", "upload"]), None));
    assert!(!remote::writes(&s(&["jj", "status"]), None));
    assert!(remote::writes(&s(&["jj", "my-alias"]), None));
}

#[test]
fn sudo_policy_classification() {
    assert!(sudo::command_approval(&s(&["id"])));
    assert!(sudo::interactive(&s(&["id"])));
    assert!(sudo::command_approval(&s(&["-u", "root", "id"])));
    assert!(sudo::command_approval(&s(&["FOO=1", "id"])));
    assert!(!sudo::command_approval(&s(&["-n", "id"])));
    assert!(!sudo::command_approval(&s(&["-l"])));
    assert!(!sudo::command_approval(&s(&["-k"])));
    assert!(!sudo::command_approval(&s(&["-h"])));
    assert!(!sudo::command_approval(&s(&["--help"])));
    assert!(!sudo::command_approval(&s(&["--version"])));
    assert!(!sudo::command_approval(&s(&[])));
    // -A / -S require command approval, but skip askpass injection.
    assert!(sudo::command_approval(&s(&["-A", "id"])));
    assert!(!sudo::interactive(&s(&["-A", "id"])));
    assert!(sudo::command_approval(&s(&["-S", "id"])));
    assert!(!sudo::interactive(&s(&["-S", "id"])));
    // Ambiguous long prefix fails closed to non-applicable (left for sudo to diagnose).
    assert!(!sudo::command_approval(&s(&["--no", "id"])));
    assert!(sudo::command_approval(&s(&["--login", "id"])));
    assert!(sudo::interactive(&s(&["--login", "id"])));
    assert!(sudo::valid_password("secret-123"));
    assert!(!sudo::valid_password("bad\nline"));
    assert!(!sudo::valid_password("bad\rline"));
}

#[test]
fn gpg_option_table_and_argument_parsing() {
    let table = "\
pinentry-mode:0:2:set pinentry mode\n\
status-fd:0:1:write status to fd\n\
command-fd:0:1:read commands from fd\n\
passphrase:0:2:use passphrase\n\
armor:97:0:create ascii armored output\n\
detach-sign:98:0:make a detached signature\n\
local-user:117:2:use user id\n";
    let options = gpg::parse_option_table(table).expect("valid option table");

    let parsed = gpg::parse_arguments(
        &s(&["--status-fd=2", "-ab", "-u", "alice", "file.txt"]),
        &options,
    );
    assert_eq!(parsed.interactive, true);
    assert_eq!(parsed.status_fd, Some(2));
    assert_eq!(parsed.forward, ["-ab", "-u", "alice", "file.txt"]);

    // Caller-owned auth flags fall back to native exec.
    let owned = gpg::parse_arguments(&s(&["--pinentry-mode", "cancel", "--sign"]), &options);
    assert_eq!(owned.interactive, false);

    // Unknown flag falls back to native exec.
    let unknown = gpg::parse_arguments(&s(&["--unknown-flag", "--sign"]), &options);
    assert_eq!(unknown.interactive, false);

    assert!(gpg::valid_passphrase("good-passphrase"));
    assert!(!gpg::valid_passphrase("bad\0nul"));
    assert!(!gpg::valid_passphrase("bad\nline"));
}
