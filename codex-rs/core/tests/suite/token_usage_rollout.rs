//! Verifies observed Responses API usage is durably recorded in rollout history.

use anyhow::Result;
use codex_history::RolloutItem;
use codex_protocol::SessionId;
use codex_protocol::protocol::TokenUsageRecord;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;

fn token_usage_records(path: &std::path::Path) -> Vec<TokenUsageRecord> {
    std::fs::read_to_string(path)
        .expect("read rollout")
        .lines()
        .filter_map(|line| codex_rollout::parse_rollout_line(line).ok())
        .filter_map(|line| match line.item {
            RolloutItem::TokenUsageRecord(record) => Some(record),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_response_usage_accumulates_per_turn_and_thread() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let plan_args = json!({
        "plan": [{
            "step": "keep sampling",
            "status": "in_progress"
        }]
    })
    .to_string();
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("response-a"),
                ev_function_call("call-a", "update_plan", &plan_args),
                ev_completed_with_tokens("response-a", /*total_tokens*/ 120),
            ]),
            sse(vec![
                ev_response_created("response-b"),
                ev_assistant_message("message-b", "done"),
                ev_completed_with_tokens("response-b", /*total_tokens*/ 80),
            ]),
            sse(vec![
                ev_response_created("response-c"),
                ev_assistant_message("message-c", "next"),
                ev_completed_with_tokens("response-c", /*total_tokens*/ 30),
            ]),
            sse(vec![
                ev_response_created("response-without-usage"),
                ev_assistant_message("message-d", "no usage"),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "response-without-usage"
                    }
                }),
            ]),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let home = test.home.clone();

    test.submit_turn("first").await?;
    test.codex.shutdown_and_wait().await?;

    let resumed = test_codex()
        .resume(&server, home, rollout_path.clone())
        .await?;
    for prompt in ["second", "third"] {
        resumed.submit_turn(prompt).await?;
    }
    resumed.codex.shutdown_and_wait().await?;

    let records = token_usage_records(&rollout_path);
    assert_eq!(records.len(), 3);
    assert_eq!(
        records
            .iter()
            .map(|record| {
                (
                    record.response_id.as_str(),
                    record.turn_token_usage.total_tokens,
                    record.thread_token_usage.total_tokens,
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("response-a", 120, 120),
            ("response-b", 200, 200),
            ("response-c", 30, 230),
        ]
    );
    assert_eq!(records[0].turn_id, records[1].turn_id);
    assert_ne!(records[1].turn_id, records[2].turn_id);
    assert!(records.iter().all(|record| {
        record.session_id == SessionId::from(record.thread_id)
            && record.root_turn_id == record.turn_id
    }));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn context_usage_notice_reaches_sampled_input_and_survives_resume() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("first"),
                ev_assistant_message("a", "done"),
                ev_completed_with_tokens("first", 60_000),
            ]),
            sse(vec![
                ev_response_created("second"),
                ev_assistant_message("b", "done"),
                ev_completed_with_tokens("second", 60_000),
            ]),
            sse(vec![
                ev_response_created("third"),
                ev_assistant_message("c", "done"),
                ev_completed_with_tokens("third", 60_000),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_context_window = Some(100_000);
            config.model_auto_compact_token_limit = Some(99_000);
            config.model_post_turn_compact_threshold_percent = 0;
        })
        .build(&server)
        .await?;
    test.submit_turn("first").await?;
    test.submit_turn("second").await?;
    let home = test.home.clone();
    let path = test.codex.rollout_path().unwrap();
    test.codex.shutdown_and_wait().await?;
    let resumed = test_codex().resume(&server, home, path).await?;
    resumed.submit_turn("third").await?;
    resumed.codex.shutdown_and_wait().await?;
    let requests = responses.requests();
    assert_eq!(requests.len(), 3);
    for (index, request) in requests.iter().enumerate() {
        let notices: Vec<_> = request
            .input()
            .into_iter()
            .filter(|item| {
                item["role"] == "developer" && item.to_string().contains("Mekugi context usage:")
            })
            .collect();
        assert_eq!(notices.len(), usize::from(index > 0));
        if index > 0 {
            assert!(notices[0].to_string().contains("60000/"));
            assert!(!notices[0].to_string().contains("system-reminder"));
        }
    }
    Ok(())
}
