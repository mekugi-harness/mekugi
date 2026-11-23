use super::super::PreviousSectionState;
use super::super::test_support::render_section_cases;
use super::*;

#[test]
fn snapshots() {
    use PreviousSectionState::Absent;
    use PreviousSectionState::Known;
    use PreviousSectionState::Unknown;

    let empty = AgentsMdState::default();
    let project_formatter = LoadedAgentsMd::from_text_for_testing("use the project formatter");
    let project_formatter = AgentsMdState::new(Some(&project_formatter));
    let old = LoadedAgentsMd::from_text_for_testing("old instructions");
    let old = AgentsMdState::new(Some(&old));
    let new = LoadedAgentsMd::from_text_for_testing("new instructions");
    let new = AgentsMdState::new(Some(&new));

    insta::assert_snapshot!(render_section_cases(&[
        (Absent, Absent),
        (Absent, Known(&empty)),
        (Absent, Known(&project_formatter)),
        (Known(&project_formatter), Known(&project_formatter)),
        (Known(&old), Known(&new)),
        (Known(&new), Known(&empty)),
        (Unknown, Known(&new)),
        (Unknown, Known(&empty)),
    ]));
}

#[test]
fn delivers_unified_diff_for_incremental_edits() {
    use pretty_assertions::assert_eq;

    let initial_text = [
        "# Project Rules",
        "",
        "## Formatting",
        "Use cargo fmt before committing and keep imports grouped consistently across crates.",
        "",
        "## Architecture",
        "Keep runtime behavior in its native owner rather than duplicating logic across layers.",
        "",
        "## Testing",
        "Run unit tests for every touched crate before opening a pull request.",
        "",
        "## Style",
        "Prefer small cohesive modules with explicit visibility and clear documentation.",
    ]
    .join("\n");
    let edited_text = initial_text.replace(
        "Run unit tests for every touched crate before opening a pull request.",
        "Run unit and integration tests for every touched crate before opening a pull request.",
    );

    let initial_loaded = LoadedAgentsMd::from_text_for_testing(&initial_text);
    let initial_state = AgentsMdState::new(Some(&initial_loaded));
    let (initial_snapshot, _) = initial_state.render_diff(PreviousSectionState::Absent);
    let initial_snapshot = initial_snapshot.expect("initial snapshot");

    let edited_loaded = LoadedAgentsMd::from_text_for_testing(&edited_text);
    let edited_state = AgentsMdState::new(Some(&edited_loaded));
    let (edited_snapshot, updates) =
        edited_state.render_diff(PreviousSectionState::Known(&initial_snapshot));
    let edited_snapshot = edited_snapshot.expect("edited snapshot");
    assert_eq!(edited_snapshot.text.as_deref(), Some(edited_text.as_str()));
    assert_eq!(updates.len(), 1);

    let rendered = render_section_cases(&[(
        PreviousSectionState::Known(&initial_state),
        PreviousSectionState::Known(&edited_state),
    )]);
    assert!(
        rendered.contains(DIFF_NOTICE),
        "expected diff notice:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "-Run unit tests for every touched crate before opening a pull request.\n+Run unit and integration tests for every touched crate before opening a pull request."
        ),
        "expected unified diff hunk:\n{rendered}"
    );
    assert!(
        !rendered.contains("Use cargo fmt before committing"),
        "unchanged distant sections should not be repeated:\n{rendered}"
    );
}

#[test]
fn directory_only_change_does_not_repeat_unchanged_text() {
    let text = "Keep responses concise.\nAlways run tests.";
    let before = AgentsMdState {
        instructions: Some(UserInstructions {
            directory: Some("/repo/a".to_string()),
            text: text.to_string(),
        }),
    };
    let after = AgentsMdState {
        instructions: Some(UserInstructions {
            directory: Some("/repo/b".to_string()),
            text: text.to_string(),
        }),
    };

    let rendered = render_section_cases(&[(
        PreviousSectionState::Known(&before),
        PreviousSectionState::Known(&after),
    )]);
    assert!(rendered.contains("# AGENTS.md instructions for /repo/b"));
    assert!(rendered.contains(UNCHANGED_NOTICE));
    assert!(!rendered.contains("Keep responses concise."));
}
