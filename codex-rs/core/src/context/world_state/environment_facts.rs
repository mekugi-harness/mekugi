use crate::context::environment_context::push_text_element;
use crate::context::environment_context::push_xml_escaped_text;
use codex_git_utils::GitEnvironmentFacts;
use codex_git_utils::git_environment_facts;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Semaphore;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
// Keep the permit with the blocking worker until it exits, including after timeout or turn drop.
static FACT_PROBES: Semaphore = Semaphore::const_new(1);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(super) struct EnvironmentFacts {
    pub(super) cwd: AbsolutePathBuf,
    // Snapshots from before structured rendering have only a preformatted text field.
    #[serde(default)]
    pub(super) data: Option<CapturedEnvironmentFacts>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) restricted_reads: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) session_tmp: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(super) struct CapturedEnvironmentFacts {
    pub(super) platform: String,
    pub(super) home: Option<String>,
    pub(super) os: Option<String>,
    pub(super) kernel: Option<String>,
    pub(super) git: GitEnvironmentFacts,
}

impl EnvironmentFacts {
    #[tracing::instrument(skip_all)]
    pub(super) async fn acquire(cwd: &AbsolutePathBuf) -> Self {
        let data = tokio::time::timeout(PROBE_TIMEOUT, probe(cwd.clone()))
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                host_facts(
                    /*os*/ None,
                    /*kernel*/ None,
                    GitEnvironmentFacts::Unavailable,
                )
            });
        Self {
            cwd: cwd.clone(),
            data: Some(data),
            restricted_reads: false,
            session_tmp: None,
        }
    }

    pub(super) fn restricted(cwd: &AbsolutePathBuf) -> Self {
        Self {
            cwd: cwd.clone(),
            data: Some(host_facts(
                /*os*/ None,
                kernel_version(),
                GitEnvironmentFacts::Unavailable,
            )),
            restricted_reads: true,
            session_tmp: None,
        }
    }

    pub(super) fn render(&self, out: &mut String) {
        let Some(data) = &self.data else {
            return;
        };
        out.push_str("  <environment_facts>\n");
        push_fact(out, "    ", "platform", &data.platform);
        push_fact(
            out,
            "    ",
            "home",
            data.home.as_deref().unwrap_or("unavailable"),
        );
        push_fact(
            out,
            "    ",
            "os",
            data.os.as_deref().unwrap_or("unavailable"),
        );
        push_fact(
            out,
            "    ",
            "kernel",
            data.kernel.as_deref().unwrap_or("unavailable"),
        );
        data.render_git(out, self.restricted_reads);
        if let Some(path) = &self.session_tmp {
            out.push_str("    <session_tmp env_var=\"SESSION_TMP\" on_path=\"true\">");
            push_xml_escaped_text(out, &path.to_string_lossy());
            out.push_str("</session_tmp>\n");
        }
        out.push_str("  </environment_facts>\n");
    }
}

impl CapturedEnvironmentFacts {
    fn render_git(&self, out: &mut String, restricted_reads: bool) {
        let git = match &self.git {
            GitEnvironmentFacts::Unavailable => {
                out.push_str(if restricted_reads {
                    "    <git_repository status=\"restricted_reads\" />\n"
                } else {
                    "    <git_repository status=\"unavailable\" />\n"
                });
                return;
            }
            GitEnvironmentFacts::NotRepository => {
                out.push_str("    <git_repository status=\"absent\" />\n");
                return;
            }
            GitEnvironmentFacts::Repository(git) => git,
        };
        out.push_str("    <git_repository status=\"present\">\n");
        if let Some(worktree) = &git.worktree {
            push_fact(out, "      ", "worktree", worktree);
        }
        if git.linked_worktree {
            push_fact(out, "      ", "linked_worktree", "true");
        }
        push_fact(out, "      ", "branch", &git.branch);
        push_fact(out, "      ", "head", &git.head);
        push_fact(
            out,
            "      ",
            "default_branch",
            git.default_branch.as_deref().unwrap_or("unavailable"),
        );
        push_fact(
            out,
            "      ",
            "hooks_path",
            git.hooks_path.as_deref().unwrap_or("unset (Git default)"),
        );
        match &git.status {
            None => out.push_str("      <status state=\"unavailable\" />\n"),
            Some(status) if status.entries.is_empty() => {
                out.push_str("      <status state=\"clean\" />\n");
            }
            Some(status) => {
                out.push_str(if status.truncated {
                    "      <status truncated=\"true\">\n"
                } else {
                    "      <status>\n"
                });
                for entry in &status.entries {
                    out.push_str("        <change code=\"");
                    push_xml_escaped_text(out, &entry.code);
                    out.push_str("\">");
                    push_xml_escaped_text(out, &entry.path);
                    out.push_str("</change>\n");
                }
                out.push_str("      </status>\n");
            }
        }
        if let Some(history) = &git.history {
            match &history.remote_branches_containing_head {
                None => out
                    .push_str("      <remote_branches_containing_head status=\"unavailable\" />\n"),
                Some(branches) if branches.entries.is_empty() => {
                    out.push_str("      <remote_branches_containing_head />\n");
                }
                Some(branches) => {
                    out.push_str(if branches.truncated {
                        "      <remote_branches_containing_head truncated=\"true\">\n"
                    } else {
                        "      <remote_branches_containing_head>\n"
                    });
                    for branch in &branches.entries {
                        push_fact(out, "        ", "branch", branch);
                    }
                    out.push_str("      </remote_branches_containing_head>\n");
                }
            }
            match &history.recent_commits {
                None => out.push_str("      <recent_commits status=\"unavailable\" />\n"),
                Some(commits) => {
                    out.push_str("      <recent_commits>\n");
                    for commit in commits {
                        out.push_str("        <commit id=\"");
                        push_xml_escaped_text(out, &commit.id);
                        out.push_str("\">");
                        push_xml_escaped_text(out, &commit.subject);
                        out.push_str("</commit>\n");
                    }
                    out.push_str("      </recent_commits>\n");
                }
            }
            match history.more_unpushed_commits {
                Some(Ok(count)) => {
                    push_fact(out, "      ", "more_unpushed_commits", &count.to_string())
                }
                Some(Err(())) => {
                    out.push_str("      <more_unpushed_commits status=\"unavailable\" />\n")
                }
                None => {}
            }
        }
        out.push_str("    </git_repository>\n");
    }
}

fn push_fact(out: &mut String, indent: &str, name: &str, value: &str) {
    out.push_str(indent);
    push_text_element(out, name, value);
    out.push('\n');
}

#[tracing::instrument(skip_all)]
async fn probe(cwd: AbsolutePathBuf) -> Option<CapturedEnvironmentFacts> {
    struct InterruptOnDrop(Arc<AtomicBool>);
    impl Drop for InterruptOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    let permit = FACT_PROBES.acquire().await.ok()?;
    let interrupt = Arc::new(AtomicBool::new(false));
    let _cancel = InterruptOnDrop(Arc::clone(&interrupt));
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        host_facts(
            os_version(),
            kernel_version(),
            git_environment_facts(&cwd, &interrupt),
        )
    })
    .await
    .ok()
}

fn host_facts(
    os: Option<String>,
    kernel: Option<String>,
    git: GitEnvironmentFacts,
) -> CapturedEnvironmentFacts {
    CapturedEnvironmentFacts {
        platform: std::env::consts::OS.to_string(),
        home: std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|home| home.to_string_lossy().into_owned()),
        os,
        kernel,
        git,
    }
}

#[cfg(target_os = "linux")]
fn os_version() -> Option<String> {
    use std::io::Read;

    let file = std::fs::File::open("/etc/os-release")
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                std::fs::File::open("/usr/lib/os-release")
            } else {
                Err(error)
            }
        })
        .ok()?;
    let mut data = String::new();
    file.take(16 * 1024 + 1).read_to_string(&mut data).ok()?;
    if data.len() > 16 * 1024 {
        return None;
    }
    data.lines()
        .rev()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(value)
        })
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(target_os = "macos")]
fn os_version() -> Option<String> {
    let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    Some(format!(
        "{}.{}.{}",
        version.majorVersion, version.minorVersion, version.patchVersion,
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn os_version() -> Option<String> {
    None
}

fn kernel_version() -> Option<String> {
    #[cfg(unix)]
    {
        let kernel = rustix::system::uname();
        Some(format!(
            "{} {}",
            kernel.sysname().to_string_lossy(),
            kernel.release().to_string_lossy(),
        ))
    }
    #[cfg(not(unix))]
    {
        None
    }
}
