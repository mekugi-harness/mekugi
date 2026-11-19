//! Shared native MAv2 child preparation and spawning.
//!
//! Callers supply captured step settings; the configured agent backend owns child lifetimes.

use crate::agent::api::AgentInput;
use crate::agent::api::SpawnRequest;
use crate::agent::child_config::SpawnConfigOptions;
use crate::agent::child_config::SpawnConfigVersion;
use crate::agent::child_config::prepare_agent_spawn_config;
use crate::agent::child_config::prepare_spawn_environments;
use crate::agent::helper_daybreak::turn::explicit_program_for_spawn;
use crate::agent::next_thread_spawn_depth;
use crate::agent::types::AgentMessage;
use crate::agent::types::LiveAgent;
use crate::agent::types::MessageDeliveryMode;
use crate::agent::types::SpawnAgentForkMode;
use crate::agent::types::SpawnAgentOptions;
use crate::agent::types::SpawnProgramSource;
use crate::codex_thread::ThreadConfigSnapshot;
use crate::config::Config;
use crate::session::multi_agents::resolve_usage_hints;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use codex_prompts::ResolvedModelMessages;
use codex_protocol::AgentPath;
use codex_protocol::error::CodexErr;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::ReloadSelection;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;

/// Child creation options; omitted overrides retain native MAv2 inheritance and role rules.
pub struct ThreadCreateRequest {
    pub task_name: String,
    /// `None` starts fresh; `FullHistory` uses native fork preparation.
    pub fork_mode: Option<SpawnAgentForkMode>,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub cwd: Option<String>,
    pub preload: Option<ReloadSelection>,
}

pub(crate) struct ThreadSpawnRequest {
    pub task_name: String,
    /// Explicit encoding: hosts normally provide plaintext; native tools can forward ciphertext.
    pub message: AgentMessage,
    /// `None` starts fresh; `FullHistory` uses native fork preparation.
    pub fork_mode: Option<SpawnAgentForkMode>,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub cwd: Option<String>,
    pub preload: Option<ReloadSelection>,
}

impl ThreadSpawnRequest {
    pub(crate) fn into_parts(
        self,
    ) -> Result<(ThreadCreateRequest, AgentMessage), ThreadSpawnError> {
        let message_text = match &self.message {
            AgentMessage::Plaintext(text) | AgentMessage::Encrypted(text) => text,
        };
        if message_text.trim().is_empty() {
            return Err(ThreadSpawnError::Preparation(
                "Empty message can't be sent to an agent".to_string(),
            ));
        }
        Ok((
            ThreadCreateRequest {
                task_name: self.task_name,
                fork_mode: self.fork_mode,
                agent_type: self.agent_type,
                model: self.model,
                reasoning_effort: self.reasoning_effort,
                cwd: self.cwd,
                preload: self.preload,
            },
            self.message,
        ))
    }
}

pub(crate) struct ChildSpawnSettings {
    pub(crate) config: Config,
    pub(crate) source: SessionSource,
    pub(crate) options: SpawnAgentOptions,
}

/// Child runtime and its effective configuration.
pub struct ThreadSpawnResult {
    pub agent: LiveAgent,
    pub config: ThreadConfigSnapshot,
    pub task_name: AgentPath,
}

/// Separates preparation errors from backend failures to preserve native tool error reporting.
#[derive(Debug, thiserror::Error)]
pub enum ThreadSpawnError {
    #[error("{0}")]
    Preparation(String),
    #[error(transparent)]
    Backend(#[from] CodexErr),
}

pub(crate) async fn spawn(
    session: &Session,
    step_context: &StepContext,
    call_id: &str,
    request: ThreadSpawnRequest,
) -> Result<ThreadSpawnResult, ThreadSpawnError> {
    let (request, message) = request.into_parts()?;
    let (settings, task_name) = prepare(session, step_context, call_id, request).await?;
    let request = SpawnRequest {
        caller: session.thread_id,
        config: settings.config,
        input: AgentInput::Message {
            message,
            mode: MessageDeliveryMode::TriggerTurn,
        },
        source: settings.source,
        options: settings.options,
    };
    let (agent, config) = session.services.agent_control.spawn(request).await?;
    Ok(ThreadSpawnResult {
        agent,
        config,
        task_name,
    })
}

/// Prepare the native child settings without choosing its coordination backend.
pub(crate) async fn prepare(
    session: &Session,
    step_context: &StepContext,
    call_id: &str,
    request: ThreadCreateRequest,
) -> Result<(ChildSpawnSettings, AgentPath), ThreadSpawnError> {
    let turn = &step_context.turn;
    let ThreadCreateRequest {
        task_name,
        fork_mode,
        agent_type,
        model,
        reasoning_effort,
        cwd,
        preload,
    } = request;
    if let Some(selection) = &preload {
        let error = if fork_mode.is_some() {
            Some(
                "Full-history forks cannot preload sources; omit preload or start fresh."
                    .to_string(),
            )
        } else {
            crate::context_sources::first_invalid_reference(&selection.sources).map(|index| {
                format!(
                    "Invalid preload.sources[{index}]: use @path for files, $skill for skills, or mchanges:ID for changes. No child was started."
                )
            })
        };
        if let Some(error) = error {
            session.services.local_agent_runtime.record_feature_use(
                session.thread_id,
                call_id,
                codex_protocol::feature_use::FeatureUseEvent::PreloadRejected,
            );
            return Err(ThreadSpawnError::Preparation(error));
        }
    }
    let role_name = agent_type
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());
    let prepared = prepare_agent_spawn_config(
        session,
        step_context,
        SpawnConfigOptions {
            version: SpawnConfigVersion::V2,
            full_history_fork: matches!(fork_mode, Some(SpawnAgentForkMode::FullHistory)),
            role_name,
            model: model.as_deref(),
            reasoning_effort,
        },
    )
    .await
    .map_err(ThreadSpawnError::Preparation)?;
    let mut config = prepared.config;
    let environments = prepare_spawn_environments(step_context, cwd.as_deref(), &mut config)
        .await
        .map_err(ThreadSpawnError::Preparation)?;
    if fork_mode.is_some() {
        config.multi_agent_v2.preserve_fork_prefix = true;
    }
    let task_name = turn
        .session_source
        .get_agent_path()
        .unwrap_or_else(AgentPath::root)
        .join(&task_name)
        .map_err(ThreadSpawnError::Preparation)?;
    let spawn_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id: session.thread_id,
        depth: next_thread_spawn_depth(&turn.session_source),
        agent_path: Some(task_name.clone()),
        agent_nickname: None,
        agent_role: prepared.role_name,
    });
    let multi_agent_v2_usage_hints = if matches!(fork_mode, Some(SpawnAgentForkMode::FullHistory))
        && turn.multi_agent_version == MultiAgentVersion::V2
    {
        let child_model_info = match config.model.as_deref() {
            Some(model) if model != turn.model_info().slug => Some(
                session
                    .services
                    .models_manager
                    .get_model_info(model, &config.to_models_manager_config())
                    .await,
            ),
            _ => None,
        };
        let child_multi_agent_messages = ResolvedModelMessages::from_model(
            child_model_info.as_ref().unwrap_or(turn.model_info()),
        )
        .multi_agent();
        Some(resolve_usage_hints(
            &config.multi_agent_v2,
            child_multi_agent_messages,
            !config.update_plan_enabled && config.model_catalog.is_none(),
        ))
    } else {
        None
    };
    Ok((
        ChildSpawnSettings {
            config,
            source: spawn_source,
            options: SpawnAgentOptions {
                fork_parent_spawn_call_id: fork_mode.as_ref().map(|_| call_id.to_string()),
                fork_mode,
                parent_thread_id: Some(session.thread_id),
                parent_turn_id: Some(turn.sub_id.clone()),
                root_turn_id: turn.turn_metadata_state.root_turn_id(),
                turn_trigger: turn.turn_metadata_state.current_turn_trigger(),
                environments: Some(environments),
                preload,
                disabled_plugin_ids: Some(turn.disabled_plugin_ids.clone()),
                multi_agent_v2_usage_hints,
                cyber_access_program: explicit_program_for_spawn(turn),
                cyber_access_program_source: SpawnProgramSource::ParentTurn,
            },
        },
        task_name,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_login::CodexAuth;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn request() -> ThreadCreateRequest {
        ThreadCreateRequest {
            task_name: "worker".to_string(),
            fork_mode: None,
            agent_type: None,
            model: None,
            reasoning_effort: None,
            cwd: None,
            preload: None,
        }
    }

    #[tokio::test]
    async fn full_history_rejects_preload_and_preserves_native_prefix() {
        let (session, turn) = crate::session::tests::make_session_and_context().await;
        let step = StepContext::for_test(Arc::new(turn));
        assert!(
            prepare(
                &session,
                &step,
                "fork",
                ThreadCreateRequest {
                    fork_mode: Some(SpawnAgentForkMode::FullHistory),
                    preload: Some(ReloadSelection { sources: vec![] }),
                    ..request()
                }
            )
            .await
            .is_err()
        );
        let (settings, _) = prepare(
            &session,
            &step,
            "fork",
            ThreadCreateRequest {
                fork_mode: Some(SpawnAgentForkMode::FullHistory),
                ..request()
            },
        )
        .await
        .unwrap();
        assert!(settings.config.multi_agent_v2.preserve_fork_prefix);
        assert_eq!(settings.options.preload, None);
    }
    #[tokio::test]
    async fn native_cwd_preparation_preserves_parent_and_captured_permissions() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("recipient")).unwrap();
        std::fs::write(directory.path().join("not-directory"), "text").unwrap();
        let (session, turn, _events) =
            crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
                CodexAuth::from_api_key("test"),
                Vec::new(),
                |config| {
                    config.cwd = AbsolutePathBuf::from_absolute_path(directory.path()).unwrap();
                    config
                        .features
                        .enable(codex_features::Feature::MultiAgentV2)
                        .unwrap();
                },
            )
            .await;
        let step = session
            .capture_step_context(turn, &CancellationToken::new())
            .await
            .unwrap();
        let parent = step.environments.primary().unwrap();
        let parent_cwd = parent.cwd().clone();
        let parent_config = parent.config().clone();
        let (prepared, _) = prepare(
            &session,
            &step,
            "selected-cwd",
            ThreadCreateRequest {
                cwd: Some("recipient".to_string()),
                ..request()
            },
        )
        .await
        .unwrap();
        let child = prepared
            .options
            .environments
            .as_ref()
            .unwrap()
            .primary()
            .unwrap();
        assert_eq!(
            child.cwd().to_abs_path().unwrap().as_path(),
            directory.path().join("recipient")
        );
        assert_eq!(child.config(), &parent_config);
        assert_eq!(parent.cwd(), &parent_cwd);
        assert_eq!(
            child.config_origin,
            crate::environment_selection::EnvironmentConfigOrigin::Owner
        );
        for cwd in ["", "missing", "not-directory"] {
            assert!(
                prepare(
                    &session,
                    &step,
                    "invalid-cwd",
                    ThreadCreateRequest {
                        cwd: Some(cwd.to_string()),
                        ..request()
                    }
                )
                .await
                .is_err()
            );
        }
        assert_eq!(parent.cwd(), &parent_cwd);
    }

    #[tokio::test]
    async fn native_cwd_preparation_rejects_denied_directories() {
        use crate::config::PermissionProfileSnapshot;
        use crate::environment_selection::TurnEnvironmentState;
        use codex_protocol::models::PermissionProfile;
        use codex_protocol::permissions::FileSystemAccessMode;
        use codex_protocol::permissions::FileSystemPath;
        use codex_protocol::permissions::FileSystemSandboxEntry;

        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("denied")).unwrap();
        let (session, turn, _events) =
            crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
                CodexAuth::from_api_key("test"),
                Vec::new(),
                |config| {
                    config.cwd = AbsolutePathBuf::from_absolute_path(directory.path()).unwrap();
                },
            )
            .await;
        let mut step = StepContext::for_test(turn);
        let profile = PermissionProfile::read_only();
        let mut policy = profile.file_system_sandbox_policy();
        policy.entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::GlobPattern {
                pattern: format!("{}/denied", directory.path().display()),
            },
            access: FileSystemAccessMode::Deny,
            missing_path_behavior: None,
        });
        let step_mut = Arc::get_mut(&mut step).unwrap();
        let TurnEnvironmentState::Ready(environment) = &mut step_mut.environments.environments[0]
        else {
            panic!("ready parent environment");
        };
        environment.config_mut().permission_profile = PermissionProfileSnapshot::legacy(
            PermissionProfile::from_runtime_permissions_with_enforcement(
                profile.enforcement(),
                &policy,
                profile.network_sandbox_policy(),
            ),
        );
        step_mut.inherited_environments = step_mut.environments.clone();
        assert!(
            prepare(
                &session,
                &step,
                "denied-cwd",
                ThreadCreateRequest {
                    cwd: Some("denied".to_string()),
                    ..request()
                }
            )
            .await
            .is_err()
        );
        assert_eq!(session.reload_selection().await, None);
    }
}
