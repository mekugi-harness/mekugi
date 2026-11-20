use super::*;
use crate::session::TurnInput;
use crate::session::UserInputMetadata;
use codex_protocol::models::ResponseItem;
use codex_thread_store::PersistContext;
use codex_user_input::ResolvedUserInput;
use pretty_assertions::assert_eq;

async fn record(session: &Arc<Session>, step: &Arc<StepContext>, text: &str) {
    let input = TurnInput::UserInput {
        content: vec![ResolvedUserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }],
        client_id: None,
        metadata: UserInputMetadata::default(),
    };
    assert!(
        !crate::session::turn::run_hooks_and_record_inputs(
            session,
            &step.turn,
            &step.settings.model_info,
            &[input],
            PersistContext::Standard,
        )
        .await
        .unwrap()
    );
}

fn outputs(history: &crate::context_manager::ContextManager) -> Vec<String> {
    history
        .raw_items()
        .filter_map(|item| match item {
            ResponseItem::FunctionCallOutput {
                name: Some(name),
                output,
                ..
            } if name == "attachments" => Some(serde_json::to_string(&output.body).unwrap()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn accepted_input_reads_at_recording_and_retains_original_sources() {
    let (directory, session, step) = tests::fixture().await;
    let path = directory.path().join("file");
    std::fs::write(&path, "QUEUED_BYTES").unwrap();
    let input = "Read @file @missing";
    std::fs::write(&path, "RECORDED_BYTES").unwrap();
    record(&session, &step, input).await;
    let first = outputs(&session.clone_history().await);
    assert_eq!(first.len(), 1);
    assert!(first[0].contains("RECORDED_BYTES"));
    assert!(first[0].contains("CONTENT NOT ATTACHED"));
    assert!(!first[0].contains("QUEUED_BYTES"));
    std::fs::write(&path, "NEW_EXPLICIT_BYTES").unwrap();
    record(&session, &step, "Continue without another source reference").await;
    assert_eq!(outputs(&session.clone_history().await), first);
    record(&session, &step, "Read @file again").await;
    let later = outputs(&session.clone_history().await);
    assert_eq!(later.len(), 2);
    assert_eq!(later[0], first[0]);
    assert!(later[1].contains("NEW_EXPLICIT_BYTES"));
}

#[tokio::test]
async fn directories_keep_ignored_depth_bounded_name_trees() {
    let (directory, session, step) = tests::fixture().await;
    let root = directory.path();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".gitignore"), "ignored\n").unwrap();
    std::fs::write(root.join("ignored"), "IGNORED_BODY").unwrap();
    std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
    std::fs::write(root.join("nested/.gitignore"), "excluded\n").unwrap();
    std::fs::write(root.join("nested/excluded"), "EXCLUDED_BODY").unwrap();
    std::fs::write(root.join("nested/child"), "NO_FILE_BODIES").unwrap();
    std::fs::write(root.join("nested/deeper/too_deep"), "DEEP_BODY").unwrap();
    std::fs::write(root.join("@literal"), "LITERAL_BODY").unwrap();
    record(&session, &step, "Read @.").await;
    let result = outputs(&session.clone_history().await).join("");
    assert!(result.contains("- nested/"));
    assert!(result.contains("- child"));
    assert!(result.contains("- deeper/"));
    assert!(result.contains("- @literal"));
    for absent in [
        "- ignored",
        "- excluded",
        "- .git/",
        "too_deep",
        "NO_FILE_BODIES",
        "LITERAL_BODY",
        "CONTENT NOT ATTACHED",
    ] {
        assert!(!result.contains(absent), "{result}");
    }
    for i in 0..300 {
        std::fs::write(root.join(format!("entry{i:03}")), "").unwrap();
    }
    record(&session, &step, "Read @.").await;
    let result = outputs(&session.clone_history().await).pop().unwrap();
    assert!(result.contains("TRUNCATED"));
    assert!(result.len() < 20_000);
}

#[test]
fn explicit_skill_sources_replace_only_the_matching_stock_prompt() {
    assert_eq!(
        message_skill_sources(r"Use $review and $review/references/a.md, not \$ignored")
            .collect::<Vec<_>>(),
        ["review", "review/references/a.md"]
    );
}
