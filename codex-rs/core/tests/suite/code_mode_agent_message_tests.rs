//! Native collaboration through code mode and model-visible requests.

use super::super::code_mode::custom_tool_output_last_non_empty_text;
use std::time::Duration;

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::AgentPath;
use codex_protocol::items::CollabAgentTool;
use codex_protocol::items::TurnItem;
use codex_protocol::openai_models::ToolMode;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_custom_tool_call;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::responses::strip_metadata_from_json;
use core_test_support::responses::strip_response_item_ids_from_json;
use core_test_support::skip_if_no_network;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use test_case::test_case;
use wiremock::matchers::header;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn yielded_code_mode_spawn_preserves_metadata_and_plaintext() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let test = test_codex()
        .with_model("gpt-5.6-sol")
        .with_model_info_override("gpt-5.6-sol", |model| {
            model.tool_mode = Some(ToolMode::CodeMode);
            model.use_responses_lite = true;
            model
                .experimental_supported_tools
                .push("test_sync_tool".to_string());
        })
        .with_config(|config| {
            for feature in [
                Feature::Collab,
                Feature::MultiAgentV2,
                Feature::CodeMode,
                Feature::CodeModeHostGrpc,
            ] {
                config
                    .features
                    .enable(feature)
                    .expect("enable test feature");
            }
            config.multi_agent_v2.non_code_mode_only = false;
        })
        .build_with_auto_env(&server)
        .await?;
    let root_header = test.session_configured.thread_id.to_string();
    let mut created = test.thread_manager.subscribe_thread_created();
    // Keep the old cell blocked until the next turn is executing, then keep that
    // turn active until the old cell has spawned its child.
    let start_barrier = r#"await tools.test_sync_tool({barrier: {
        id: "spawn-origin", participants: 2, timeout_ms: 60000
    }});"#;

    mount_sse_once_match(
        &server,
        header("thread-id", root_header.as_str()),
        sse(vec![
            ev_response_created("spawn"),
            ev_custom_tool_call(
                "spawn-worker",
                "exec",
                &format!(r#"yield_control();
{start_barrier}
text(await tools.collaboration__spawn_agent({{task_name: "worker", message: "Summarize the three stories.", fork_turns: "all"}}));"#),
            ),
            ev_completed("spawn"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        header("thread-id", root_header.as_str()),
        sse(vec![
            ev_assistant_message("yielded", "Waiting for the next turn."),
            ev_completed("yielded"),
        ]),
    )
    .await;
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "Delegate the summary task.".to_string(),
                text_elements: Vec::new(),
            }])
            .with_responses_metadata(Some(HashMap::from([(
                "cost_center".to_string(),
                "origin".to_string(),
            )]))),
        )
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    mount_sse_once_match(
        &server,
        header("thread-id", root_header.as_str()),
        sse(vec![
            ev_custom_tool_call("release-worker", "exec", start_barrier),
            ev_completed("release-worker"),
        ]),
    )
    .await;
    // Wait outside nested tool dispatch: another test_sync_tool barrier could
    // hold shared admission while the spawn needs exclusive admission.
    mount_sse_once_match(
        &server,
        header("thread-id", root_header.as_str()),
        sse(vec![
            ev_function_call("wait-worker", "wait", r#"{"cell_id":"1"}"#),
            ev_completed("wait-worker"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        header("thread-id", root_header.as_str()),
        sse(vec![
            ev_assistant_message("delegated", "The worker started."),
            ev_completed("delegated"),
        ]),
    )
    .await;
    let child_requests = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            request
                .headers
                .get("thread-id")
                .is_some_and(|id| id != root_header.as_str())
        },
        sse(vec![ev_response_created("child"), ev_completed("child")]),
    )
    .await;
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "Release the previous turn's cell.".to_string(),
                text_elements: Vec::new(),
            }])
            .with_responses_metadata(Some(HashMap::from([(
                "cost_center".to_string(),
                "current".to_string(),
            )]))),
        )
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let child_id = created.try_recv()?;
    let child = test.thread_manager.get_thread(child_id).await?;
    wait_for_event(&child, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let child_request = child_requests
        .requests()
        .into_iter()
        .find(|request| request.header("thread-id") == Some(child_id.to_string()))
        .expect("child receives its task");
    let child_metadata: Value = serde_json::from_str(
        child_request.body_json()["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("canonical child metadata"),
    )?;
    assert_eq!(child_metadata["cost_center"], "origin");
    assert_eq!(
        strip_response_item_ids_from_json(strip_metadata_from_json(Value::Array(
            child_request.inputs_of_type("agent_message"),
        ))),
        json!([{
            "type": "agent_message",
            "author": "/root",
            "recipient": "/root/worker",
            "content": [{
                "type": "input_text",
                "text": "Message Type: NEW_TASK\nTask name: /root/worker\nSender: /root\nPayload:\nSummarize the three stories.",
            }],
        }]),
    );
    assert_eq!(test.thread_manager.list_thread_ids().await.len(), 2);
    Ok(())
}

#[test_case(true; "partial_read")]
#[test_case(false; "all_reads_fail")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_spawn_can_retry_malformed_preload(readable: bool) -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let test = test_codex()
        .with_model("gpt-5.6-sol")
        .with_model_info_override("gpt-5.6-sol", |model| {
            model.tool_mode = Some(ToolMode::CodeMode);
            model.use_responses_lite = true;
        })
        .with_config(|config| {
            for feature in [
                Feature::Collab,
                Feature::MultiAgentV2,
                Feature::CodeMode,
                Feature::CodeModeHost,
                Feature::CodeModeHostGrpc,
            ] {
                config.features.enable(feature).unwrap();
            }
            config.multi_agent_v2.non_code_mode_only = false;
        })
        .with_code_mode_host_program(codex_utils_cargo_bin::cargo_bin("codex-code-mode-host")?)
        .build_with_auto_env(&server)
        .await?;
    let root = test.session_configured.thread_id;
    let root_header = root.to_string();
    let child_match_root = root_header.clone();
    let mut created = test.thread_manager.subscribe_thread_created();
    let child_dir = test.cwd_path().join("delegate");
    std::fs::create_dir(&child_dir)?;
    std::fs::write(test.cwd_path().join("brief.txt"), "PARENT_BRIEF")?;
    if readable {
        std::fs::write(child_dir.join("brief.txt"), "CHILD_BRIEF")?;
    }
    let child_requests = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            request
                .headers
                .get("thread-id")
                .is_some_and(|id| id != child_match_root.as_str())
        },
        sse(vec![
            ev_assistant_message("child", "Done."),
            ev_completed("child"),
        ]),
    )
    .await;

    for (call_id, program, prompt) in [
        (
            "malformed",
            r#"
const errors = [];
for (const invalid of ["brief.txt", "/brief.txt", "@brief.txt#L0"]) {
  try {
    await tools.collaboration__spawn_agent({
      task_name: "worker", message: "Read the brief.", fork_turns: "none",
      cwd: "delegate", preload: {sources: ["@missing.txt", invalid]}
    });
  } catch (error) {
    errors.push(String(error));
  }
}
text(errors.join("\n"));
"#,
            "Try a malformed preload.",
        ),
        (
            "corrected",
            r#"text(await tools.collaboration__spawn_agent({
  task_name: "worker", message: "Read the brief.", fork_turns: "none",
  cwd: "delegate", preload: {sources: ["@missing.txt", "@brief.txt"]}
}));"#,
            "Retry with valid references.",
        ),
    ] {
        let initial = mount_sse_once_match(
            &server,
            header("thread-id", root_header.as_str()),
            sse(vec![
                ev_custom_tool_call(call_id, "exec", program),
                ev_completed(call_id),
            ]),
        )
        .await;
        let finished_id = format!("{call_id}-done");
        let finished = mount_sse_once_match(
            &server,
            header("thread-id", root_header.as_str()),
            sse(vec![
                ev_assistant_message(&finished_id, "Checked."),
                ev_completed(&finished_id),
            ]),
        )
        .await;
        test.submit_turn(prompt).await?;
        let request = finished.single_request();
        let output =
            custom_tool_output_last_non_empty_text(&request, call_id).unwrap_or_else(|| {
                panic!(
                    "{call_id} has no exec text: {}",
                    request.custom_tool_call_output(call_id)
                )
            });
        if call_id == "malformed" {
            assert!(
                initial
                    .single_request()
                    .body_contains_text("Bare paths are invalid.")
            );
            assert_eq!(
                output.matches("Invalid preload.sources[1]").count(),
                3,
                "{output}"
            );
            assert!(output.contains("use @path for files"), "{output}");
            assert!(matches!(
                created.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert!(child_requests.requests().iter().all(|request| {
                request.header("thread-id").as_deref() == Some(root_header.as_str())
            }));
        } else {
            assert!(output.contains("/root/worker"), "{output}");
        }
    }

    let child_id = created.try_recv()?;
    let child = test.thread_manager.get_thread(child_id).await?;
    wait_for_event_with_timeout(
        &child,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(15),
    )
    .await;
    let child_header = child_id.to_string();
    let mut requests = child_requests
        .requests()
        .into_iter()
        .filter(|request| request.header("thread-id").as_deref() == Some(child_header.as_str()));
    let child_request = requests
        .next()
        .expect("child receives the corrected assignment");
    assert!(
        requests.next().is_none(),
        "expected exactly one child inference"
    );
    assert_eq!(child_request.body_contains_text("CHILD_BRIEF"), readable);
    assert!(!child_request.body_contains_text("PARENT_BRIEF"));
    assert!(child_request.body_contains_text(r#"Source "@missing.txt": CONTENT NOT ATTACHED:"#));
    assert_eq!(
        child_request.body_contains_text(r#"Source "@brief.txt": CONTENT NOT ATTACHED:"#),
        !readable,
    );
    assert_eq!(test.thread_manager.list_thread_ids().await.len(), 2);
    Ok(())
}

#[test_case(false, false; "initial_execute")]
#[test_case(true, false; "subsequent_wait")]
#[test_case(true, true; "interruption")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_wait_survives_grpc_outcome_deadline(
    reobserve: bool,
    interrupt: bool,
) -> Result<()> {
    skip_if_no_network!(Ok(()));

    let prepare = if reobserve {
        "yield_control();"
    } else {
        r#"await tools.team__spawn_agent({ task_name: "worker", message: "Complete after release.", fork_turns: "none" });"#
    };
    let source = format!(
        "// @exec: {{\"yield_time_ms\": 500}}\nconst marker = 'original cell'; {prepare} text(await tools.team__wait_agent({{}})); text(marker);"
    );
    let chunk = |events| {
        vec![StreamingSseChunk {
            gate: None,
            body: sse(events),
        }]
    };
    let mut streams = vec![chunk(vec![
        ev_response_created("exec"),
        ev_custom_tool_call("native-wait", "exec", &source),
        ev_completed("exec"),
    ])];
    if reobserve {
        streams.push(chunk(vec![
            ev_response_created("observe"),
            ev_function_call(
                "observe-wait",
                "wait",
                r#"{"cell_id":"1","yield_time_ms":500}"#,
            ),
            ev_completed("observe"),
        ]));
    }
    let (release_child, child_gate) = tokio::sync::oneshot::channel();
    if !reobserve {
        streams.push(vec![
            StreamingSseChunk {
                gate: None,
                body: sse(vec![ev_response_created("child")]),
            },
            StreamingSseChunk {
                gate: Some(child_gate),
                body: sse(vec![
                    ev_assistant_message("child-final", "Child released after deadline."),
                    ev_completed("child"),
                ]),
            },
        ]);
    }
    let pending_requests = streams.len();
    streams.push(chunk(vec![
        ev_assistant_message("done", "Received."),
        ev_completed("done"),
    ]));
    let (server, _) = start_streaming_sse_server(streams).await;
    let test = test_codex()
        .with_model("gpt-5.6-sol")
        .with_config(|config| {
            config.multi_agent_v2.tool_namespace = Some("team".into());
            config.multi_agent_v2.non_code_mode_only = false;
            for feature in [
                Feature::Collab,
                Feature::MultiAgentV2,
                Feature::CodeMode,
                Feature::CodeModeHost,
                Feature::CodeModeHostGrpc,
            ] {
                config.features.enable(feature).unwrap();
            }
        })
        .build_with_streaming_server(&server)
        .await?;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Wait for the worker.".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_event_with_timeout(&test.codex, |event| matches!(event, EventMsg::ItemStarted(event)
        if matches!(&event.item, TurnItem::CollabAgentToolCall(call) if call.tool == CollabAgentTool::Wait)), Duration::from_secs(15)).await;
    server.wait_for_request_count(pending_requests).await;
    if interrupt {
        test.codex.submit(Op::Interrupt).await?;
        wait_for_event_with_timeout(
            &test.codex,
            |event| matches!(event, EventMsg::TurnAborted(_)),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(server.requests().await.len(), pending_requests);
        server.shutdown().await;
        return Ok(());
    }
    assert!(
        tokio::time::timeout(
            // 500 ms requested yield + 1s grace + 60s transport budget.
            Duration::from_secs(63),
            server.wait_for_request_count(pending_requests + 1)
        )
        .await
        .is_err(),
        "observer or gRPC deadline started another model request"
    );
    if reobserve {
        test.codex
            .submit(Op::InterAgentCommunication {
                communication: InterAgentCommunication::new(
                    AgentPath::root().join("worker").expect("valid worker path"),
                    AgentPath::root(),
                    Vec::new(),
                    "Worker is ready.".into(),
                    false,
                ),
                start_options: Default::default(),
            })
            .await?;
    } else {
        release_child
            .send(())
            .expect("child inference is still pending");
    }
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let requests = server.requests().await;
    assert_eq!(requests.len(), pending_requests + 1);
    let body: Value = serde_json::from_slice(&requests[pending_requests])?;
    let id = if reobserve {
        "observe-wait"
    } else {
        "native-wait"
    };
    let output = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["call_id"] == id
                && item["type"]
                    .as_str()
                    .is_some_and(|kind| kind.ends_with("_output"))
        })
        .unwrap();
    assert!(
        output["output"].to_string().contains("Wait completed.")
            && output["output"].to_string().contains("original cell"),
        "{output}"
    );
    server.shutdown().await;
    Ok(())
}
