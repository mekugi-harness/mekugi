use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ServerRequestResolvedNotification;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::openai_models::ReasoningEffort;
use core_test_support::responses;
use serde_json::json;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const RED_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
const BLUE_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYPgPAAEDAQAIicLsAAAAAElFTkSuQmCC";

fn create_request_user_input_sse_response(
    call_id: &str,
    with_images: bool,
) -> anyhow::Result<String> {
    let mut questions = vec![json!({
            "header": "Confirm",
            "question": "Proceed with the plan?",
            "options": [{
                "label": "Yes (Recommended)",
                "description": "Continue the current plan."
            }, {
                "label": "No",
                "description": "Stop and revisit the approach."
            }]
    })];
    if with_images {
        questions.push(json!({"header":"Example", "question":"Show a second example."}));
    }
    let tool_call_arguments = serde_json::to_string(&json!({ "questions": questions }))?;

    Ok(responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_function_call(call_id, "request_user_input", &tool_call_arguments),
        responses::ev_completed("resp-1"),
    ]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_user_input_round_trip() -> Result<()> {
    request_user_input_round_trip_for_mode(
        ModeKind::Plan,
        /*enable_default_mode_feature*/ false,
        /*expected_is_blocking*/ true,
        /*image_answers*/ None,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_user_input_default_mode_forwards_non_blocking() -> Result<()> {
    request_user_input_round_trip_for_mode(
        ModeKind::Default,
        /*enable_default_mode_feature*/ true,
        /*expected_is_blocking*/ false,
        /*image_answers*/ None,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_user_input_delivers_typed_images_for_their_questions() -> Result<()> {
    request_user_input_round_trip_for_mode(
        ModeKind::Plan,
        /*enable_default_mode_feature*/ false,
        /*expected_is_blocking*/ true,
        Some([RED_PNG, BLUE_PNG]),
    )
    .await
}

async fn request_user_input_round_trip_for_mode(
    mode: ModeKind,
    enable_default_mode_feature: bool,
    expected_is_blocking: bool,
    image_answers: Option<[&str; 2]>,
) -> Result<()> {
    let codex_home = tempfile::TempDir::new()?;
    let responses = vec![
        create_request_user_input_sse_response("call1", image_answers.is_some())?,
        create_final_assistant_message_sse_response("done")?,
    ];
    let server = create_mock_responses_server_sequence(responses).await;
    MockResponsesConfig::new(&server.uri())
        .with_approval_policy("on-request")
        .write(codex_home.path())?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            config: enable_default_mode_feature.then(|| {
                std::collections::HashMap::from([(
                    "features.default_mode_request_user_input".to_string(),
                    json!(true),
                )])
            }),
            ..Default::default()
        })
        .await?;

    let TurnStartResponse { turn, .. } = mcp
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                client_user_message_id: None,
                input: vec![V2UserInput::Text {
                    text: "ask something".to_string(),
                    text_elements: Vec::new(),
                }],
                model: Some("mock-model".to_string()),
                effort: Some(ReasoningEffort::Medium),
                collaboration_mode: Some(CollaborationMode {
                    mode,
                    settings: Settings {
                        model: "mock-model".to_string(),
                        reasoning_effort: Some(ReasoningEffort::Medium),
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let server_req = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_request_message(),
    )
    .await??;
    let ServerRequest::ToolRequestUserInput { request_id, params } = server_req else {
        panic!("expected ToolRequestUserInput request, got: {server_req:?}");
    };

    assert_eq!(params.thread_id, thread.id);
    assert_eq!(params.turn_id, turn.id);
    assert_eq!(params.item_id, "call1");
    assert_eq!(
        params.questions.len(),
        if image_answers.is_some() { 2 } else { 1 }
    );
    assert_eq!(params.is_blocking, expected_is_blocking);
    assert_eq!(params.auto_resolution_ms, None);
    let resolved_request_id = request_id.clone();

    let response = if let Some([red, blue]) = image_answers {
        json!({"answers": [
            {"id": "q2", "images": [blue], "other": "second"},
            {"id": "q1", "images": [red], "other": ""}
        ]})
    } else {
        json!({"answers": [{"id":"q1","choice":1}]})
    };
    mcp.send_response(request_id, response).await?;
    let mut saw_resolved = false;
    loop {
        let message = timeout(DEFAULT_READ_TIMEOUT, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        match notification.method.as_str() {
            "serverRequest/resolved" => {
                let resolved: ServerRequestResolvedNotification = serde_json::from_value(
                    notification
                        .params
                        .clone()
                        .expect("serverRequest/resolved params"),
                )?;
                assert_eq!(resolved.thread_id, thread.id);
                assert_eq!(resolved.request_id, resolved_request_id);
                saw_resolved = true;
            }
            "turn/completed" => {
                assert!(saw_resolved, "serverRequest/resolved should arrive first");
                break;
            }
            _ => {}
        }
    }

    if let Some([red, blue]) = image_answers {
        let requests = server.received_requests().await.expect("recorded requests");
        let provider_input = requests[1].body_json::<serde_json::Value>()?;
        let output = provider_input["input"]
            .as_array()
            .expect("provider input")
            .iter()
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == "call1")
            .expect("question tool output");
        let parts = output["output"].as_array().expect("typed tool output");
        let text = parts[0]["text"].as_str().expect("compact answer text");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text)?,
            json!([{"id":"q1","other":""}, {"id":"q2","other":"second"}])
        );
        assert_eq!(parts.len(), 5);
        assert_eq!(
            parts[1],
            json!({"type":"input_text","text":"Images for answer q1:"})
        );
        assert_eq!(parts[2]["type"], "input_image");
        assert_eq!(parts[2]["image_url"], red);
        assert_eq!(
            parts[3],
            json!({"type":"input_text","text":"Images for answer q2:"})
        );
        assert_eq!(parts[4]["type"], "input_image");
        assert_eq!(parts[4]["image_url"], blue);
    }

    Ok(())
}
