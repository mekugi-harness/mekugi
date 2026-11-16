use crate::exec::{ExecCapturePolicy, execute_exec_request};
use crate::sandboxing::ExecRequest;
use codex_protocol::models::PermissionProfile;
use codex_sandboxing::SandboxType;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(super) struct EnvironmentFacts {
    pub(super) cwd: AbsolutePathBuf,
    pub(super) text: String,
}

impl EnvironmentFacts {
    pub(super) async fn acquire(cwd: &AbsolutePathBuf) -> Self {
        let output = execute_exec_request(
            ExecRequest::new(
                [
                    "bash",
                    "-pc",
                    include_str!("environment_facts.sh"),
                    std::env::consts::OS,
                ]
                .map(str::to_owned)
                .to_vec(),
                cwd.clone(),
                std::env::vars()
                    .filter(|(key, _)| !key.starts_with("GIT_") || key.starts_with("GIT_CONFIG_"))
                    .collect(),
                None,
                None,
                3000_u64.into(),
                ExecCapturePolicy::ShellTool,
                SandboxType::None,
                Vec::new(),
                codex_protocol::config_types::WindowsSandboxLevel::Disabled,
                PermissionProfile::Disabled,
                None,
            ),
            None,
            None,
        )
        .await;
        let text = output
            .ok()
            .filter(|output| output.exit_code == 0)
            .map(|output| output.stdout.text)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME")
                    .ok()
                    .filter(|home| !home.is_empty())
                    .unwrap_or_else(|| "unavailable".to_owned());
                format!(
                    "Observed at: {}\nPlatform: {}\nOS: unavailable\nKernel: unavailable\nHome: {home:?}\nGit repository: unavailable\n",
                    chrono::Utc::now().to_rfc3339(), std::env::consts::OS,
                )
            });
        Self {
            cwd: cwd.clone(),
            text,
        }
    }
}
