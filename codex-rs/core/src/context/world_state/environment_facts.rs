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
    pub(super) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) session_tmp: Option<PathBuf>,
}

impl EnvironmentFacts {
    #[tracing::instrument(skip_all)]
    pub(super) async fn acquire(cwd: &AbsolutePathBuf) -> Self {
        let text = tokio::time::timeout(PROBE_TIMEOUT, probe(cwd.clone()))
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                host_facts(/*os*/ None, /*kernel*/ None) + "Git repository: unavailable\n"
            });
        Self {
            cwd: cwd.clone(),
            text,
            session_tmp: None,
        }
    }
}

#[tracing::instrument(skip_all)]
async fn probe(cwd: AbsolutePathBuf) -> Option<String> {
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
        let mut text = host_facts(os_version().as_deref(), kernel_version().as_deref());
        text.push_str(&codex_git_utils::git_environment_facts(&cwd, &interrupt));
        text
    })
    .await
    .ok()
}

fn host_facts(os: Option<&str>, kernel: Option<&str>) -> String {
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty());
    let platform = std::env::consts::OS;
    let os_label = if cfg!(target_os = "macos") {
        "OS (macOS)"
    } else {
        "OS"
    };
    let home = home.as_deref().map(std::ffi::OsStr::to_string_lossy);
    let home = home.as_deref().unwrap_or("unavailable");
    let os = os.unwrap_or("unavailable");
    let kernel = kernel.unwrap_or("unavailable");
    format!("Platform: {platform}\nHome: {home}\n{os_label}: {os}\nKernel: {kernel}\n")
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
