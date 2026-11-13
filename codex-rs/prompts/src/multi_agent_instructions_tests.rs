//! Covers role composition and attribution for captured model instructions.

use super::*;
use codex_context_fragments::AnnotatedContent;
use codex_context_fragments::RenderedFragment;
use codex_protocol::models::ContentItemMetadata;
use pretty_assertions::assert_eq;

#[test]
fn role_segment_filters_base_and_appends_bundled_guidance() {
    let shared = DEFAULT_MULTI_AGENT_V2_SHARED_USAGE_HINT_TEXT;
    let wait = DEFAULT_MULTI_AGENT_V2_WAIT_AGENT_USAGE_HINT_TEXT;
    let model_override = DEFAULT_MULTI_AGENT_V2_MODEL_OVERRIDE_USAGE_HINT_TEXT;
    let expected_body = format!(
        "Role.\n## Work\nContinue.\n{shared}\n{wait}\n\nThere are 2 available concurrency slots, meaning that up to 2 agents can be active at once, including you.\n\n{model_override}"
    );
    for marked in [false, true] {
        let instructions = MultiAgentRoleInstructions::Composed {
            base: "Role.\n## Plan tool\nOmit role checklist guidance.\n## Work\nContinue."
                .to_string(),
            marked,
            omit_update_plan_instructions: true,
            max_concurrency: 2,
            wait_agent_enabled: true,
            expose_model_overrides: true,
        };
        let expected_text = if marked {
            format!("<multi_agent_role>{expected_body}</multi_agent_role>")
        } else {
            expected_body.clone()
        };
        assert_eq!(
            instructions.render_fragment(),
            RenderedFragment::new(
                "developer",
                AnnotatedContent::text(
                    expected_text,
                    ContentItemKind("multi_agent.role_instructions".to_string()),
                    ContentItemMetadata::harness(),
                ),
            ),
        );
    }
}

#[test]
fn subagent_guidance_preserves_selected_role_and_attribution() {
    let configured = MultiAgentRoleInstructions::Configured("Configured role.".to_string());
    let composed = MultiAgentRoleInstructions::Composed {
        base: "Catalog role.".to_string(),
        marked: true,
        omit_update_plan_instructions: false,
        max_concurrency: 2,
        wait_agent_enabled: false,
        expose_model_overrides: false,
    };
    for instructions in [configured, composed] {
        let markers = instructions.markers();
        let base = instructions.body().into_text().unwrap();
        let instructions = instructions.with_subagent_guidance();
        let text = instructions.body().into_text().unwrap();
        assert_eq!(instructions.markers(), markers);
        assert!(text.starts_with(base.lines().next().unwrap()));
        assert!(text.contains("authenticated parent assignment carries the parent's authority"));
        assert!(text.contains("functions.request_user_input_async"));
        assert!(text.contains("Route assignment changes and blockers to your parent."));
        assert!(
            text.contains("Include exact mchanges source references in your ordinary final answer")
        );
    }
}
