use super::super::environment_facts::CapturedEnvironmentFacts;
use crate::context::world_state::test_support::FragmentSectionTestExt as _;
use crate::shell::ShellType;
use codex_git_utils::GitCommitFact;
use codex_git_utils::GitEnvironmentFacts;
use codex_git_utils::GitFactList;
use codex_git_utils::GitHistoryFacts;
use codex_git_utils::GitRepositoryFacts;
use codex_git_utils::GitStatusEntry;
use codex_protocol_test_support::ContentItemTestExt;

use super::*;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::permissions::project_roots_glob_pattern;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::test_support::PathBufExt;
use core_test_support::test_path_buf;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::path::PathBuf;

fn fake_shell_name() -> String {
    let shell = crate::shell::Shell {
        shell_type: ShellType::Bash,
        shell_path: PathBuf::from("/bin/bash"),
    };
    shell.name().to_string()
}

fn test_abs_path(unix_path: &str) -> AbsolutePathBuf {
    test_path_buf(unix_path).abs()
}

fn environment(id: &str, cwd: PathUri, shell: impl Into<String>) -> (String, EnvironmentState) {
    (
        id.to_string(),
        EnvironmentState {
            cwd,
            status: EnvironmentStatus::Available,
            error: None,
            shell: Some(shell.into()),
            is_primary: false,
            filesystem: None,
            network_access: None,
        },
    )
}

fn environment_state(
    environments: impl IntoIterator<Item = (String, EnvironmentState)>,
    current_date: Option<String>,
    timezone: Option<String>,
    network: Option<NetworkContext>,
    subagents: Option<String>,
) -> EnvironmentsState {
    let environments = environments
        .into_iter()
        .enumerate()
        .map(|(index, (id, mut environment))| {
            environment.is_primary = index == 0;
            (id, environment)
        })
        .collect();
    EnvironmentsState {
        environments,
        per_environment: false,
        shell_version: None,
        current_date,
        current_date_is_utc: false,
        timezone,
        network,
        filesystem: None,
        facts: None,
        subagents,
    }
}

#[test]
fn serialize_workspace_write_environment_context() {
    let cwd = test_path_buf("/repo");
    let context = environment_state(
        [environment(
            "local",
            PathUri::from_abs_path(&cwd.abs()),
            fake_shell_name(),
        )],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{cwd}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
</environment_context>"#,
        cwd = cwd.display(),
    );

    assert_eq!(context.render().expect_text(), expected);
}

#[tokio::test]
async fn environment_facts_render_without_a_second_timestamp() {
    let directory = tempfile::tempdir().expect("create non-repository workspace");
    let cwd = directory.path().to_path_buf().abs();
    let context = environment_state(
        [environment(
            "local",
            PathUri::from_abs_path(&cwd),
            fake_shell_name(),
        )],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    )
    .with_facts(&cwd, &PermissionProfile::Disabled, /*previous*/ None)
    .await;

    let rendered = context.render().expect_text();
    assert!(rendered.contains("<current_date>2026-02-26</current_date>"));
    assert!(rendered.contains("<timezone>America/Los_Angeles</timezone>"));
    assert!(rendered.contains(&format!(
        "  <environment_facts>\n    <platform>{}</platform>\n",
        std::env::consts::OS
    )));
    #[cfg(unix)]
    assert!(
        rendered
            .lines()
            .any(|line| line.starts_with("    <kernel>") && !line.contains("unavailable"))
    );
    assert!(
        rendered.contains("    <git_repository status=\"absent\" />\n  </environment_facts>\n")
    );
    assert!(!rendered.contains("Observed at:"));
}

#[tokio::test]
async fn legacy_prose_snapshot_gets_structured_facts() {
    let cwd = test_abs_path("/repo");
    let previous = serde_json::json!({
        "environments": {},
        "facts": {
            "cwd": cwd,
            "text": "Platform: legacy-platform\n",
            "restricted_reads": true
        }
    });
    let mut policy = FileSystemSandboxPolicy::read_only();
    policy.entries.push(FileSystemSandboxEntry::new(
        cwd.join("private").into(),
        FileSystemAccessMode::Deny,
    ));
    let permissions =
        PermissionProfile::from_runtime_permissions(&policy, NetworkSandboxPolicy::Restricted);
    let previous_facts = serde_json::from_value::<EnvironmentsSnapshot>(previous.clone())
        .expect("read legacy snapshot")
        .facts
        .expect("legacy facts existed");
    assert!(previous_facts.data.is_none());
    let context = EnvironmentsState::default()
        .with_facts(&cwd, &permissions, Some(&previous))
        .await;

    let rendered = context.render().expect_text();
    assert!(rendered.contains(r#"<git_repository status="restricted_reads" />"#));
    assert!(!rendered.contains("legacy-platform"));
}

#[test]
fn serialize_environment_facts_with_git_status_and_session_tmp() {
    let context = EnvironmentsState {
        facts: Some(EnvironmentFacts {
            cwd: test_abs_path("/repo"),
            data: Some(CapturedEnvironmentFacts {
                platform: "linux".to_string(),
                home: Some("/home/<user>".to_string()),
                os: Some("Debian".to_string()),
                kernel: Some("Linux 6".to_string()),
                git: GitEnvironmentFacts::Repository(Box::new(GitRepositoryFacts {
                    worktree: Some("/work&tree".to_string()),
                    linked_worktree: true,
                    branch: "work".to_string(),
                    head: "abcdef123".to_string(),
                    default_branch: Some("main".to_string()),
                    hooks_path: None,
                    status: Some(GitFactList {
                        entries: vec![GitStatusEntry {
                            code: "??".to_string(),
                            path: "<file>&".to_string(),
                        }],
                        truncated: true,
                    }),
                    history: Some(GitHistoryFacts {
                        remote_branches_containing_head: Some(GitFactList {
                            entries: vec!["origin/work".to_string()],
                            truncated: false,
                        }),
                        recent_commits: Some(vec![GitCommitFact {
                            id: "abcdef123".to_string(),
                            subject: "fix <tag> & text".to_string(),
                        }]),
                        more_unpushed_commits: Some(Ok(2)),
                    }),
                })),
            }),
            restricted_reads: false,
            session_tmp: None,
        }),
        ..Default::default()
    }
    .with_session_tmp(PathBuf::from("scratch & work"));

    assert_eq!(
        context.render().expect_text(),
        r#"<environment_context>
  <environment_facts>
    <platform>linux</platform>
    <home>/home/&lt;user&gt;</home>
    <os>Debian</os>
    <kernel>Linux 6</kernel>
    <git_repository status="present">
      <worktree>/work&amp;tree</worktree>
      <linked_worktree>true</linked_worktree>
      <branch>work</branch>
      <head>abcdef123</head>
      <default_branch>main</default_branch>
      <hooks_path>unset (Git default)</hooks_path>
      <status truncated="true">
        <change code="??">&lt;file&gt;&amp;</change>
      </status>
      <remote_branches_containing_head>
        <branch>origin/work</branch>
      </remote_branches_containing_head>
      <recent_commits>
        <commit id="abcdef123">fix &lt;tag&gt; &amp; text</commit>
      </recent_commits>
      <more_unpushed_commits>2</more_unpushed_commits>
    </git_repository>
    <session_tmp env_var="SESSION_TMP">scratch &amp; work</session_tmp>
  </environment_facts>
</environment_context>"#
    );
}

#[test]
fn serialize_environment_context_with_foreign_windows_cwd() {
    let mut context = environment_state(
        [environment(
            "remote",
            PathUri::parse("file:///C:/windows").expect("Windows cwd URI"),
            "powershell",
        )],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    context.per_environment = true;
    let environment = context.environments.get_mut("remote").unwrap();
    environment.network_access = Some(NetworkAccess::Enabled);
    environment.filesystem = Some(
        FileSystemContext::from_permission_profile(
            &PermissionProfile::Disabled,
            &[PathUri::parse("file:///D:/workspace").expect("Windows workspace root URI")],
        )
        .render(),
    );

    assert_eq!(
        context.render().expect_text(),
        r#"<environment_context>
  <environments>
    <environment id="remote">
      <cwd>C:\windows</cwd>
      <shell>powershell</shell>
      <network access="enabled" />
      <filesystem><workspace_roots><root>D:\workspace</root></workspace_roots><permission_profile type="disabled"><file_system type="unrestricted" /></permission_profile></filesystem>
    </environment>
  </environments>
</environment_context>"#
    );
}

#[test]
fn serialize_environment_context_with_network() {
    let network = NetworkContext::new(
        vec!["api.example.com".to_string(), "*.openai.com".to_string()],
        vec!["blocked.example.com".to_string()],
    );
    let context = environment_state(
        [environment(
            "local",
            PathUri::from_abs_path(&test_abs_path("/repo")),
            fake_shell_name(),
        )],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        Some(network),
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
  <network enabled="true"><allowed>api.example.com,*.openai.com</allowed><denied>blocked.example.com</denied></network>
</environment_context>"#,
        test_path_buf("/repo").display()
    );

    assert_eq!(context.render().expect_text(), expected);
}

fn workspace_write_permission_profile_with_private_denials() -> PermissionProfile {
    PermissionProfile::from_runtime_permissions(
        &FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some("private".to_string())),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::GlobPattern {
                    pattern: project_roots_glob_pattern(Path::new("private/**")),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]),
        NetworkSandboxPolicy::Restricted,
    )
}

#[test]
fn serialize_environment_context_with_full_filesystem_profile() {
    let repo = test_abs_path("/repo");
    let other_repo = test_abs_path("/other-repo");
    let repo_private = repo.join("private");
    let other_repo_private = other_repo.join("private");
    let repo_private_glob =
        AbsolutePathBuf::resolve_path_against_base(Path::new("private/**"), repo.as_path());
    let other_repo_private_glob =
        AbsolutePathBuf::resolve_path_against_base(Path::new("private/**"), other_repo.as_path());
    let mut context = environment_state(
        [environment(
            "local",
            PathUri::from_abs_path(&test_abs_path("/repo")),
            fake_shell_name(),
        )],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    context.filesystem = Some(FileSystemContext::from_permission_profile(
        &workspace_write_permission_profile_with_private_denials(),
        &[
            PathUri::from_abs_path(&repo),
            PathUri::from_abs_path(&other_repo),
        ],
    ));

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <filesystem><workspace_roots><root>{repo}</root><root>{other_repo}</root></workspace_roots><permission_profile type="managed"><file_system type="restricted"><entry access="write"><path>{repo}</path></entry><entry access="write"><path>{other_repo}</path></entry><entry access="deny" escalatable="false"><path>{repo_private}</path></entry><entry access="deny" escalatable="false"><path>{other_repo_private}</path></entry><entry access="deny" escalatable="false"><glob>{repo_private_glob}</glob></entry><entry access="deny" escalatable="false"><glob>{other_repo_private_glob}</glob></entry></file_system></permission_profile></filesystem>
</environment_context>"#,
        test_path_buf("/repo").display(),
        repo = repo.to_string_lossy(),
        other_repo = other_repo.to_string_lossy(),
        repo_private = repo_private.to_string_lossy(),
        other_repo_private = other_repo_private.to_string_lossy(),
        repo_private_glob = repo_private_glob.to_string_lossy(),
        other_repo_private_glob = other_repo_private_glob.to_string_lossy(),
    );

    assert_eq!(context.render().expect_text(), expected);
}

#[test]
fn serialize_read_only_environment_context() {
    let context = environment_state(
        Vec::new(),
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = r#"<environment_context>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
</environment_context>"#;

    assert_eq!(context.render().expect_text(), expected);
}

#[test]
fn serialize_environment_context_with_subagents() {
    let context = environment_state(
        [environment(
            "local",
            PathUri::from_abs_path(&test_abs_path("/repo")),
            fake_shell_name(),
        )],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        Some("- agent-1: atlas\n- agent-2".to_string()),
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
  <subagents>
    - agent-1: atlas
    - agent-2
  </subagents>
</environment_context>"#,
        test_path_buf("/repo").display()
    );

    assert_eq!(context.render().expect_text(), expected);
}

#[test]
fn serialize_environment_context_with_multiple_selected_environments() {
    let local_cwd = test_path_buf("/repo/local");
    let remote_cwd = test_path_buf("/repo/remote");
    let context = environment_state(
        [
            environment("local", PathUri::from_abs_path(&local_cwd.abs()), "bash"),
            environment("remote", PathUri::from_abs_path(&remote_cwd.abs()), "bash"),
        ],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <environments>
    <environment id="local" primary="true">
      <cwd>{}</cwd>
      <shell>bash</shell>
    </environment>
    <environment id="remote" primary="false">
      <cwd>{}</cwd>
      <shell>bash</shell>
    </environment>
  </environments>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
</environment_context>"#,
        local_cwd.display(),
        remote_cwd.display()
    );

    assert_eq!(context.render().expect_text(), expected);
}

#[test]
fn serialize_environment_context_prefers_environment_shell_when_present() {
    let local_cwd = test_path_buf("/repo/local");
    let remote_cwd = test_path_buf("/repo/remote");
    let context = environment_state(
        [
            environment(
                "local",
                PathUri::from_abs_path(&local_cwd.abs()),
                "powershell",
            ),
            environment("remote", PathUri::from_abs_path(&remote_cwd.abs()), "cmd"),
        ],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <environments>
    <environment id="local" primary="true">
      <cwd>{}</cwd>
      <shell>powershell</shell>
    </environment>
    <environment id="remote" primary="false">
      <cwd>{}</cwd>
      <shell>cmd</shell>
    </environment>
  </environments>
</environment_context>"#,
        local_cwd.display(),
        remote_cwd.display()
    );

    assert_eq!(context.render().expect_text(), expected);
}

fn powershell_environment() -> EnvironmentsState {
    let cwd = PathUri::from_abs_path(&test_abs_path("/repo"));
    EnvironmentsState {
        environments: [environment("local", cwd, "powershell")].into(),
        shell_version: Some("5.1".to_string()),
        ..Default::default()
    }
}

#[test]
fn shell_version_diff_restates_shell_from_legacy_snapshot() {
    let current = powershell_environment();
    let mut previous = current
        .render_fragment_diff(PreviousSectionState::Absent)
        .0
        .unwrap();
    previous.shell_version = None;
    previous.environments.get_mut("local").expect("local").shell = None;
    let rendered = current
        .render_fragment_diff(PreviousSectionState::Known(&previous))
        .1
        .expect("shell version update")
        .render()
        .expect_text();
    assert!(
        rendered.contains("<shell>powershell</shell>\n  <shell_version>5.1</shell_version>"),
        "{rendered}"
    );
}

#[test]
fn shell_version_diff_clears_previously_visible_version() {
    let previous = powershell_environment();
    let current = EnvironmentsState {
        shell_version: None,
        ..previous.clone()
    };
    assert_eq!(
        current
            .render_fragment_diff(PreviousSectionState::Known(
                &previous
                    .render_fragment_diff(PreviousSectionState::Absent)
                    .0
                    .unwrap()
            ))
            .1
            .expect("removed shell version")
            .render()
            .expect_text(),
        "<environment_context>\n  <shell_version status=\"unavailable\" />\n</environment_context>"
    );
}

#[test_case::test_case(false; "local")]
#[test_case::test_case(true; "utc")]
fn current_date_diff_clears_once_and_recovers(current_date_is_utc: bool) {
    let available = EnvironmentsState {
        current_date: Some("2026-06-17".to_string()),
        current_date_is_utc,
        ..Default::default()
    };
    let tag = if current_date_is_utc {
        "current_date_utc"
    } else {
        "current_date"
    };
    let unavailable = EnvironmentsState::default();
    assert_eq!(
        unavailable
            .render_fragment_diff(PreviousSectionState::Known(
                &available
                    .render_fragment_diff(PreviousSectionState::Absent)
                    .0
                    .unwrap()
            ))
            .1
            .expect("removed current date")
            .render()
            .expect_text(),
        format!(
            "<environment_context>\n  <{tag} status=\"unavailable\" />\n</environment_context>"
        )
    );
    assert!(
        unavailable
            .render_fragment_diff(PreviousSectionState::Known(
                &unavailable
                    .render_fragment_diff(PreviousSectionState::Absent)
                    .0
                    .unwrap()
            ))
            .1
            .is_none()
    );
    assert_eq!(
        available
            .render_fragment_diff(PreviousSectionState::Known(
                &unavailable
                    .render_fragment_diff(PreviousSectionState::Absent)
                    .0
                    .unwrap()
            ))
            .1
            .expect("restored current date")
            .render()
            .expect_text(),
        format!("<environment_context>\n  <{tag}>2026-06-17</{tag}>\n</environment_context>")
    );
}
