use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::config_types::ModeKind;
use codex_protocol::openai_models::ReasoningEffort;

pub fn builtin_collaboration_mode_presets() -> Vec<CollaborationModeMask> {
    vec![
        CollaborationModeMask {
            name: ModeKind::Plan.display_name().to_string(),
            mode: Some(ModeKind::Plan),
            model: None,
            reasoning_effort: Some(Some(ReasoningEffort::Medium)),
            developer_instructions: Some(None),
        },
        CollaborationModeMask {
            name: ModeKind::Default.display_name().to_string(),
            mode: Some(ModeKind::Default),
            model: None,
            reasoning_effort: None,
            developer_instructions: Some(None),
        },
    ]
}

#[cfg(test)]
#[path = "collaboration_mode_presets_tests.rs"]
mod tests;
