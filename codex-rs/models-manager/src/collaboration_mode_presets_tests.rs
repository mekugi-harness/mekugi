use super::*;
use pretty_assertions::assert_eq;

#[test]
fn preset_names_use_mode_display_names() {
    let presets = builtin_collaboration_mode_presets();
    let [plan, default] = presets.as_slice() else {
        panic!("expected Plan and Default presets");
    };
    assert_eq!(plan.name, ModeKind::Plan.display_name());
    assert_eq!(default.name, ModeKind::Default.display_name());
    assert_eq!(plan.model, None);
    assert_eq!(plan.reasoning_effort, Some(Some(ReasoningEffort::Medium)));
    assert_eq!(default.model, None);
    assert_eq!(default.reasoning_effort, None);
}
