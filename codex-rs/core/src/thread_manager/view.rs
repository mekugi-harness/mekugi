//! Shared construction of native thread views.
use crate::ThreadConfigSnapshot;
use crate::path_utils;
use codex_app_server_protocol::{
    Thread, ThreadStatus, ThreadSection, ThreadSectionAppearance, GitInfo as ApiGitInfo,
};
use codex_thread_store::StoredThread;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_protocol::ThreadId;
use std::path::PathBuf;
use tracing::warn;

pub fn thread_from_stored_thread(
    thread: StoredThread,
    fallback_provider: &str,
    fallback_cwd: &AbsolutePathBuf,
) -> (Thread, Option<codex_thread_store::StoredThreadHistory>) {
    let path = thread.rollout_path;
    let git_info = thread.git_info.map(|info| ApiGitInfo {
        sha: info.commit_hash.map(|sha| sha.0),
        branch: info.branch,
        origin_url: info.repository_url.map(String::from),
    });
    let cwd = AbsolutePathBuf::relative_to_current_dir(path_utils::normalize_for_native_workdir(
        thread.cwd,
    ))
    .unwrap_or_else(|err| {
        warn!("failed to normalize thread cwd while reading stored thread: {err}");
        fallback_cwd.clone()
    });
    let source = with_thread_spawn_agent_metadata(
        thread.source,
        thread.agent_nickname.clone(),
        thread.agent_role.clone(),
    );
    let history = thread.history;
    let thread_id = thread.thread_id.to_string();
    let thread = Thread {
        id: thread_id.clone(),
        environments: None,
        extra: None,
        session_id: thread_id,
        forked_from_id: thread.forked_from_id.map(|id| id.to_string()),
        attached_to_thread_id: thread.attached_to_thread_id.map(|id| id.to_string()),
        parent_thread_id: thread.parent_thread_id.map(|id| id.to_string()),
        preview: thread.preview,
        ephemeral: false,
        section: thread.section.map(|section| ThreadSection {
            id: section.id,
            name: section.name,
            appearance: section
                .appearance
                .map(|appearance| ThreadSectionAppearance {
                    icon: appearance.icon,
                    color: appearance.color,
                }),
        }),
        section_entered_at: thread
            .section_entered_at
            .map(|entered_at| entered_at.timestamp()),
        project_id: thread.project_id,
        history_mode: thread.history_mode.into(),
        model_provider: if thread.model_provider.is_empty() {
            fallback_provider.to_string()
        } else {
            thread.model_provider
        },
        model: thread.model,
        reasoning_effort: thread.reasoning_effort,
        service_tier: None,
        context_tokens: None,
        context_window: None,
        completed_compactions: None,
        agent_status: None,

        created_at: thread.created_at.timestamp(),
        updated_at: thread.updated_at.timestamp(),
        recency_at: Some(thread.recency_at.timestamp()),
        status: ThreadStatus::NotLoaded,
        path,
        cwd,
        cli_version: thread.cli_version,
        originator: thread.originator,
        agent_nickname: source.get_nickname(),
        agent_role: source.get_agent_role(),
        source: source.into(),
        can_accept_direct_input: None,
        thread_source: thread.thread_source.map(Into::into),
        git_info,
        name: thread.name,
        daybreak_enabled: thread.daybreak_enabled,
        turns: Vec::new(),
    };
    (thread, history)
}

pub fn build_thread_from_snapshot(
    thread_id: ThreadId,
    session_id: String,
    multi_agent_version: Option<codex_protocol::protocol::MultiAgentVersion>,
    config_snapshot: &ThreadConfigSnapshot,
    path: Option<PathBuf>,
) -> Thread {
    let now = chrono::Utc::now().timestamp();
    Thread {
        id: thread_id.to_string(),
        environments: Some(
            config_snapshot
                .environment_selections()
                .iter()
                .map(Into::into)
                .collect(),
        ),
        extra: None,
        session_id,
        forked_from_id: None,
        attached_to_thread_id: None,
        parent_thread_id: config_snapshot.parent_thread_id.map(|id| id.to_string()),
        preview: String::new(),
        ephemeral: config_snapshot.ephemeral,
        section: None,
        section_entered_at: None,
        project_id: None,
        history_mode: config_snapshot.history_mode.into(),
        model_provider: config_snapshot.model_provider_id.clone(),
        model: Some(config_snapshot.model.clone()),
        reasoning_effort: config_snapshot.reasoning_effort.clone(),
        service_tier: None,
        context_tokens: None,
        context_window: None,
        completed_compactions: None,
        agent_status: None,

        created_at: now,
        updated_at: now,
        recency_at: Some(now),
        status: ThreadStatus::NotLoaded,
        path,
        cwd: config_snapshot.cwd().clone(),
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        originator: (!config_snapshot.originator.is_empty())
            .then(|| config_snapshot.originator.clone()),
        agent_nickname: config_snapshot.session_source.get_nickname(),
        agent_role: config_snapshot.session_source.get_agent_role(),
        source: config_snapshot.session_source.clone().into(),
        can_accept_direct_input: Some(
            config_snapshot
                .session_source
                .can_accept_direct_input(multi_agent_version),
        ),
        thread_source: config_snapshot.thread_source.clone().map(Into::into),
        git_info: None,
        name: None,
        daybreak_enabled: None,
        turns: Vec::new(),
    }
}

pub fn with_thread_spawn_agent_metadata(
    source: codex_protocol::protocol::SessionSource,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
) -> codex_protocol::protocol::SessionSource {
    if agent_nickname.is_none() && agent_role.is_none() {
        return source;
    }

    match source {
        codex_protocol::protocol::SessionSource::SubAgent(
            codex_protocol::protocol::SubAgentSource::ThreadSpawn {
                parent_thread_id,
                depth,
                agent_path,
                agent_nickname: existing_agent_nickname,
                agent_role: existing_agent_role,
            },
        ) => codex_protocol::protocol::SessionSource::SubAgent(
            codex_protocol::protocol::SubAgentSource::ThreadSpawn {
                parent_thread_id,
                depth,
                agent_path,
                agent_nickname: agent_nickname.or(existing_agent_nickname),
                agent_role: agent_role.or(existing_agent_role),
            },
        ),
        _ => source,
    }
}
