//! Native collaboration through code mode and model-visible requests.

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
use test_case::test_case;
use wiremock::matchers::header;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_spawn_delivers_plaintext() -> Result<()> {
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

    mount_sse_once_match(
        &server,
        header("thread-id", root_header.as_str()),
        sse(vec![
            ev_response_created("spawn"),
            ev_custom_tool_call(
                "spawn-worker",
                "exec",
                r#"text(await tools.collaboration__spawn_agent({task_name: "worker", message: "Summarize the three stories.", fork_turns: "all"}));"#,
            ),
            ev_completed("spawn"),
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
    test.submit_turn("Delegate the summary task.").await?;
    let child_id = created.try_recv()?;
    let child = test.thread_manager.get_thread(child_id).await?;
    wait_for_event(&child, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let child_request = child_requests
        .requests()
        .into_iter()
        .find(|request| request.header("thread-id") == Some(child_id.to_string()))
        .expect("child receives its task");
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
