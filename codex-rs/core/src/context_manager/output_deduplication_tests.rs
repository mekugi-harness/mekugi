use super::*;
use crate::context_manager::ContextManager;
use codex_attachment_store::InlineAttachmentStore;
use codex_history::CodexHarnessMetadata;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::openai_models::InputModality;
use pretty_assertions::assert_eq;
use serde_json::json;

fn tool(text: &str) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        call_id: None,
        name: Some("test".to_owned()),
        namespace: Some("functions".to_owned()),
        output: FunctionCallOutputPayload::from_text(text.to_owned()),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn output(item: &ResponseItem) -> &str {
    let ResponseItem::FunctionCallOutput { output, .. } = item else {
        panic!()
    };
    output.text_content().unwrap()
}

fn source(reference: &str, body: &str) -> String {
    let reference = serde_json::to_string(reference).unwrap();
    let payload = format!("\nSource {reference}:\n{body}\n");
    let start = payload.len() - body.len() - 1;
    format!(
        "<mekugi-source-v1 bytes=\"{}\" bodies=\"{start}:{}\">\n{payload}\n</mekugi-source-v1>",
        payload.len(),
        payload.len() - 1,
    )
}

fn call(number: usize) -> Option<Anchor> {
    Some(Anchor::call(number, 0, true))
}

fn distinct_lines(prefix: &str, count: usize) -> String {
    (0..count)
        .map(|line| format!("{prefix} {line}\n"))
        .collect()
}

async fn projected(items: Vec<ResponseItem>) -> Vec<ResponseItem> {
    let mut history = ContextManager::default();
    history.replace(items);
    history
        .for_prompt(
            &[InputModality::Text, InputModality::Image],
            &InlineAttachmentStore,
        )
        .await
}

#[tokio::test]
async fn whole_outputs_keep_first_visible_copy_and_preserve_the_request_prefix() {
    let text = distinct_lines(
        "A complete output line with enough content to save tokens.",
        8,
    );
    let mut history = ContextManager::default();
    let originals = vec![tool(&text), tool(&text), tool(&text)];
    history.replace(originals.clone());
    let full = history
        .clone()
        .for_prompt_annotated(&[InputModality::Text], &InlineAttachmentStore)
        .await;
    assert_eq!(output(&full.items[0].item), format!("[call c1]\n{text}"));
    assert_eq!(
        output(&full.items[1].item),
        "[call c2]\n[identical to call c1 above]"
    );
    assert_eq!(
        output(&full.items[2].item),
        "[call c3]\n[identical to call c1 above]"
    );
    assert_eq!(history.raw_items().cloned().collect::<Vec<_>>(), originals);
    let expected = 3 * text.len()
        - full
            .items
            .iter()
            .map(|item| output(&item.item).len())
            .sum::<usize>();
    assert_eq!(
        full.savings,
        Savings {
            bytes: expected as u64,
            tokens: expected as u64 / 4
        }
    );
    for end in 1..=3 {
        let prefix = projected(originals[..end].to_vec()).await;
        assert_eq!(
            prefix,
            full.items[..end]
                .iter()
                .map(|item| item.item.clone())
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(projected(originals.clone()).await, full.into_items());
    // A reset has no memory of the discarded occurrence.
    assert_eq!(
        output(&projected(vec![originals[2].clone()]).await[0]),
        format!("[call c1]\n{text}")
    );
}

#[test]
fn whole_matches_below_the_run_minimum_use_the_inclusive_net_savings_gate() {
    let reference = "[identical to call c1 above]";
    let overhead = "[call c2]\n".len();
    let length = reference.len() + overhead + MIN_SAVING;
    assert!(length < MIN_RUN);
    for (bytes, replaced) in [(length - 1, false), (length, true)] {
        let text = "x".repeat(bytes);
        let mut window = Window::default();
        assert_eq!(window.project(&text, call(1), 0), text);
        let actual = window.project(&text, call(2), overhead);
        assert_eq!(actual == reference, replaced);
    }
}

#[tokio::test]
async fn a_short_whole_output_can_reference_a_source_without_its_own_call_label() {
    let reference = "[identical to $x above]";
    let body = "x".repeat(reference.len() + MIN_SAVING);
    assert!(body.len() < MIN_RUN);
    let items = projected(vec![tool(&source("$x", &body)), tool(&body)]).await;
    assert_eq!(output(&items[1]), reference);
    // A short unlabelled output is not itself an addressable reuse base.
    let items = projected(vec![tool(&body), tool(&body)]).await;
    assert_eq!(output(&items[0]), body);
    assert_eq!(output(&items[1]), body);
}

#[tokio::test]
async fn line_runs_reference_earlier_body_lines_without_rewriting_that_body() {
    let shared = (0..8)
        .map(|line| format!("shared line {line:02}: {}\n", "x".repeat(36)))
        .collect::<String>();
    let first = format!("first header\n{shared}first tail\n");
    let later = format!("later header\n{shared}later tail\n");
    let items = projected(vec![tool(&first), tool(&later)]).await;
    assert_eq!(output(&items[0]), format!("[call c1]\n{first}"));
    let body = output(&items[1]);
    assert!(
        body.contains("[identical to call c1#L2-L9 above]"),
        "{body}"
    );
    assert!(body.contains("later header\n"));
    assert!(body.ends_with("later tail\n"));
}

#[test]
fn runs_require_192_exact_line_aligned_bytes_and_preserve_line_endings() {
    for length in [MIN_RUN - 1, MIN_RUN] {
        let shared = format!("{}\n", "a".repeat(length - 1));
        let mut window = Window::default();
        window.project(&format!("first\n{shared}tail"), call(1), 0);
        let later = format!("second\n{shared}other");
        let actual = window.project(&later, call(2), 0);
        assert_eq!(
            actual.contains("[identical to call c1#L2-L2 above]"),
            length == MIN_RUN
        );
    }
    let crlf = distinct_lines("exact line with Windows ending", 12).replace('\n', "\r\n");
    let lf = crlf.replace("\r\n", "\n");
    let mut window = Window::default();
    window.project(&crlf, call(1), 0);
    assert_eq!(window.project(&lf, call(2), 0), lf);
    assert_eq!(
        window.project(&crlf, call(3), 0),
        "[identical to call c1 above]"
    );
}

#[tokio::test]
async fn line_runs_inside_one_output_reference_only_earlier_literal_lines() {
    let first = distinct_lines(&"first block ".repeat(5), 5);
    let second = distinct_lines(&"second block ".repeat(5), 5);
    // A seed starting near the end of the first block would cross the first
    // replacement if indexed from the unprojected output.
    let original = format!("{first}{first}{second}{first}{second}");
    let items = projected(vec![tool(&original), tool(&first)]).await;
    assert_eq!(
        output(&items[0]),
        format!(
            "[call c1]\n{first}[identical to call c1#L1-L5 above]\n{second}[identical to call c1#L1-L5 above]\n[identical to call c1#L11-L15 above]\n"
        )
    );
    assert_eq!(
        output(&items[1]),
        "[call c2]\n[identical to call c1#L1-L5 above]"
    );
}

#[test]
fn in_output_runs_do_not_search_past_the_literal_window() {
    let run = format!("{}\n", "x".repeat(MIN_RUN - 1));
    let gap = format!("{}\n", "y".repeat(WINDOW_BYTES));
    let text = format!("{run}{gap}{run}");
    assert_eq!(Window::default().project(&text, call(1), 0), text);
}

#[test]
fn compact_diffs_win_when_smaller_than_partial_references() {
    let mut before = String::new();
    let mut after = String::new();
    for line in 0..30 {
        let common = format!("common {line:02}: {}\n", "c".repeat(192));
        before.push_str(&format!("{common}old {line:02}\n"));
        after.push_str(&format!("{common}new {line:02}\n"));
    }
    let mut window = Window::default();
    window.project(&before, call(1), 0);
    let partial = window.partial(&after, None);
    let actual = window.project(&after, call(2), 0);
    assert!(actual.contains("```diff\n"), "{actual}");
    assert!(actual.contains("-old 00\n+new 00\n"));
    assert!(actual.len() < partial.text.len());
    assert!(actual.contains("call c1#L1-L60"));
    // A rewritten body is never a whole-body base for a later reference chain.
    assert!(!window.bodies.values().any(|base| base.text == after));
}

#[test]
fn fragmented_partial_references_prefer_compact_diff_within_clarity_budget() {
    let block_a = distinct_lines("section A unchanged line with enough content for a run", 8);
    let block_b = distinct_lines("section B unchanged line with enough content for a run", 8);
    let block_c = distinct_lines("section C unchanged line with enough content for a run", 8);
    let old_1 = format!("old paragraph one: {}\n", "a".repeat(100));
    let new_1 = format!("new paragraph one: {}\n", "b".repeat(100));
    let old_2 = format!("old paragraph two: {}\n", "c".repeat(100));
    let new_2 = format!("new paragraph two: {}\n", "d".repeat(100));
    let before = format!("{block_a}{old_1}{block_b}{old_2}{block_c}");
    let after = format!("{block_a}{new_1}{block_b}{new_2}{block_c}");

    let mut window = Window::default();
    window.project(&before, call(1), 0);
    let partial = window.partial(&after, None);
    assert_eq!(partial.references, 3);
    let actual = window.project(&after, call(2), 0);
    assert!(actual.contains("```diff\n"), "{actual}");
    assert!(actual.len() > partial.text.len());
    assert!(actual.contains(&format!("-{old_1}+{new_1}")), "{actual}");
    assert!(actual.contains(&format!("-{old_2}+{new_2}")), "{actual}");
    assert!(!window.bodies.values().any(|base| base.text == after));
}

#[test]
fn single_partial_reference_does_not_yield_to_larger_diff() {
    let before = distinct_lines("unchanged log prefix line with stable output text", 16);
    let appended = distinct_lines("appended log tail line emitted later in the run", 3);
    let after = format!("{before}{appended}");

    let mut window = Window::default();
    window.project(&before, call(1), 0);
    let partial = window.partial(&after, None);
    assert_eq!(partial.references, 1);
    let actual = window.project(&after, call(2), 0);
    assert_eq!(actual, partial.text);
    assert!(!actual.contains("```diff\n"));
}

#[test]
fn fragmented_partial_references_keep_partial_when_diff_exceeds_clarity_budget() {
    // Ratio cap: diff overhead stays below MAX_DIFF_CLARITY_OVERHEAD, but drops >25% of savings.
    let small_a = distinct_lines("small section A unchanged line with stable content", 10);
    let small_b = distinct_lines("small section B unchanged line with stable content", 10);
    let old_mid = distinct_lines("deleted middle section line with old prose content", 10);
    let new_mid = distinct_lines("short replacement line in the middle", 2);
    let before_ratio = format!("{small_a}{old_mid}{small_b}");
    let after_ratio = format!("{small_a}{new_mid}{small_b}");

    let mut window = Window::default();
    window.project(&before_ratio, call(1), 0);
    let partial = window.partial(&after_ratio, None);
    assert_eq!(partial.references, 2);
    let actual = window.project(&after_ratio, call(2), 0);
    assert_eq!(actual, partial.text);
    assert!(!actual.contains("```diff\n"));

    // Absolute cap: diff retains >=75% of savings, but adds more than MAX_DIFF_CLARITY_OVERHEAD.
    let large_a = distinct_lines(
        "large section A unchanged line with enough text for a big output body",
        100,
    );
    let large_b = distinct_lines(
        "large section B unchanged line with enough text for a big output body",
        100,
    );
    let large_deleted = distinct_lines(
        "deleted block line that inflates unified diff beyond absolute cap",
        40,
    );
    let before_cap = format!("{large_a}{large_deleted}{large_b}");
    let after_cap = format!("{large_a}replacement\n{large_b}");

    let mut window = Window::default();
    window.project(&before_cap, call(1), 0);
    let partial = window.partial(&after_cap, None);
    assert_eq!(partial.references, 2);
    let actual = window.project(&after_cap, call(2), 0);
    assert_eq!(actual, partial.text);
    assert!(!actual.contains("```diff\n"));
}

#[tokio::test]
async fn sources_use_their_own_references_and_original_file_line_numbers() {
    let body = (0..20)
        .map(|line| format!("source {line:02}: {}\n", "s".repeat(48)))
        .collect::<String>();
    let first = source("@src/a.rs#L10-L19,L30-L39", &body);
    let repeated = source("$other", &body);
    let subset = body
        .split_inclusive('\n')
        .skip(5)
        .take(10)
        .collect::<String>();
    let items = projected(vec![tool(&first), tool(&repeated), tool(&subset)]).await;
    assert_eq!(output(&items[0]), first);
    let second = output(&items[1]);
    assert!(second.contains("[identical to @src/a.rs#L10-L19,L30-L39 above]"));
    assert!(!second.contains("[call"));
    let third = output(&items[2]);
    assert!(third.contains("[call c1]"));
    assert!(third.contains("@src/a.rs#L15-L19,L30-L34 above"), "{third}");
    // The projected source still has valid, updated byte boundaries.
    let mut seen = Vec::new();
    crate::context_sources::project_source_bodies(second, |reference, body| {
        seen.push((reference.to_owned(), body.to_owned()));
        body.to_owned()
    })
    .unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "$other");
}

#[tokio::test]
async fn user_text_assistant_prose_and_images_are_never_deduplicated() {
    let text = distinct_lines("literal content not owned by output deduplication", 12);
    let user: ResponseItem = serde_json::from_value(json!({
        "type": "message", "role": "user", "content": [{"type":"input_text","text":text}]
    }))
    .unwrap();
    let assistant: ResponseItem = serde_json::from_value(json!({
        "type": "message", "role": "assistant", "content": [{"type":"output_text","text":text}]
    }))
    .unwrap();
    let framed_user: ResponseItem = serde_json::from_value(json!({
        "type": "message", "role": "user", "content": [{"type":"input_text","text":source("@typed", &text)}]
    })).unwrap();
    let multipart: ResponseItem = serde_json::from_value(json!({
        "type": "function_call_output", "name": "test", "output": [
            {"type":"input_text", "text":text},
            {"type":"input_image", "image_url":"data:image/png;base64,aW1hZ2U="},
            {"type":"input_text", "text":text}
        ]
    }))
    .unwrap();
    let items = projected(vec![
        user.clone(),
        assistant.clone(),
        framed_user.clone(),
        multipart.clone(),
        multipart,
    ])
    .await;
    for (actual, expected) in items[..3].iter().zip([user, assistant, framed_user]) {
        let ResponseItem::Message {
            role: actual_role,
            content: actual,
            ..
        } = actual
        else {
            panic!()
        };
        let ResponseItem::Message {
            role: expected_role,
            content: expected,
            ..
        } = expected
        else {
            panic!()
        };
        assert_eq!(actual_role, &expected_role);
        assert_eq!(*actual, expected);
    }
    for item in &items[3..] {
        let ResponseItem::FunctionCallOutput { output, .. } = item else {
            panic!()
        };
        let parts = output.content_items().unwrap();
        assert!(
            matches!(&parts[1], FunctionCallOutputContentItem::InputImage { image, .. }
            if *image == codex_protocol::models::ImageReference::Inline { image_url: "data:image/png;base64,aW1hZ2U=".to_owned() })
        );
    }
    let ResponseItem::FunctionCallOutput { output: first, .. } = &items[3] else {
        panic!()
    };
    let parts = first.content_items().unwrap();
    assert_eq!(
        parts[0],
        FunctionCallOutputContentItem::InputText {
            text: format!("[call c1]\n{text}")
        }
    );
    assert!(
        matches!(&parts[2], FunctionCallOutputContentItem::InputText { text }
        if text.contains("call c1#L1-L12 above"))
    );
}

#[tokio::test]
async fn supplied_agent_sources_are_projected_but_the_message_is_not_a_base() {
    let text = "the same prose stays literal in agent communication\n".repeat(10);
    let framed = source("$supplied", &text);
    let message: ResponseItem = serde_json::from_value(json!({
        "type":"agent_message", "author":"/root", "recipient":"/root/child",
        "content":[{"type":"input_text","text":format!("{text}\n{framed}")}]
    }))
    .unwrap();
    let items = projected(vec![message, tool(&text), tool(&framed)]).await;
    let ResponseItem::AgentMessage { content, .. } = &items[0] else {
        panic!()
    };
    assert_eq!(
        content[0],
        AgentMessageInputContent::InputText {
            text: format!("{text}\n{framed}")
        }
    );
    assert!(output(&items[1]).contains("[identical to $supplied above]"));
    assert!(output(&items[2]).contains("[identical to $supplied above]"));
}

#[tokio::test]
async fn original_history_annotations_survive_the_model_only_projection() {
    let text = distinct_lines(
        "retained evidence remains available for inspection and replay",
        10,
    );
    let metadata = CodexHarnessMetadata {
        history_sequence: Some(7),
        sampled_through_sequence: Some(6),
        ..Default::default()
    };
    let originals = vec![
        ResponseItemEnvelope {
            item: tool(&text),
            metadata: Some(metadata.clone())
        };
        2
    ];
    let mut history = ContextManager::default();
    history.replace_annotated(originals.clone());
    let projected = history
        .clone()
        .for_prompt_annotated(&[InputModality::Text], &InlineAttachmentStore)
        .await;
    assert_eq!(history.annotated_items(), originals);
    assert_eq!(projected.items[1].metadata, Some(metadata));
    assert!(output(&projected.items[1].item).contains("[identical to call c1 above]"));
}

#[test]
fn visible_search_storage_and_indices_stay_within_the_request_window() {
    let mut window = Window::default();
    let first = distinct_lines("first literal", 64);
    window.project(&first, call(1), 0);
    for number in 2..=4000 {
        let body = format!("unique {number}: {}\n", format!("{number:06}").repeat(32));
        window.project(&body, call(number), 0);
        assert!(window.bytes <= WINDOW_BYTES);
        assert!(window.seeds.values().map(VecDeque::len).sum::<usize>() <= window.bytes);
        assert_eq!(
            window.wholes.values().map(VecDeque::len).sum::<usize>(),
            window.bodies.len()
        );
    }
    assert_eq!(window.project(&first, call(4001), 0), first);
    let large = distinct_lines("tail visible in a large output", WINDOW_BYTES / 24);
    window.project(&large, call(4002), 0);
    assert!(window.bytes <= WINDOW_BYTES);
    assert!(window.bodies.values().all(|body| !body.complete));
}

#[test]
fn non_saving_source_identities_do_not_build_repeated_candidate_lists() {
    for lines in [1_000, 10_000] {
        let text = "x\n".repeat(lines);
        let identity = format!("@{}", "long-path/".repeat(text.len() / 10 + 1));
        let mut window = Window::default();
        assert_eq!(
            window.project(&text, Some(Anchor::source(&identity)), 0),
            text
        );
        assert!(window.bodies.is_empty());
        assert!(window.seeds.is_empty());
        assert!(window.wholes.is_empty());
        assert_eq!(window.bytes, 0);
        let text = distinct_lines("x", lines);
        assert_eq!(window.project(&text, call(1), 0), text);
        assert_eq!(
            window.project(&text, call(2), 0),
            "[identical to call c1 above]"
        );
    }
}
