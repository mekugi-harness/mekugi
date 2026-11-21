//! Drive model-visible exec/wait through the real Code Mode host.

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_features::Feature;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::TruncationPolicyConfig;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;
use pretty_assertions::assert_eq;
use serde_json::Value;
use test_case::test_case;
use wiremock::MockServer;

use super::code_mode_test_builder;
use super::custom_tool_output_items;
use super::extract_running_cell_id;
use super::function_tool_output_items;
use super::pcm16_audio_data_url;
use super::run_code_mode_turn;
use super::run_code_mode_turn_with_builder;
use super::text_item;

fn status(items: &[Value]) -> &str {
    text_item(items, /*index*/ 0)
        .lines()
        .next()
        .expect("status")
}

fn terminal_cell_id<'a>(items: &'a [Value], terminal_status: &str) -> &'a str {
    status(items)
        .strip_prefix(&format!(
            "{terminal_status} (more output: wait with cell ID "
        ))
        .and_then(|rest| rest.strip_suffix(')'))
        .expect("terminal page should have a wait continuation")
}

fn body(items: &[Value]) -> String {
    (1..items.len())
        .map(|index| text_item(items, index))
        .collect()
}

async fn wait_page(
    server: &MockServer,
    test: &TestCodex,
    cell_id: &str,
    sequence: usize,
    max_tokens: usize,
    terminate: bool,
) -> Result<Vec<Value>> {
    let call_id = format!("page-{sequence}");
    let args =
        serde_json::json!({"cell_id": cell_id, "max_tokens": max_tokens, "terminate": terminate});
    let mock =
        responses::mount_function_call_agent_response(server, &call_id, &args.to_string(), "wait")
            .await;
    test.submit_turn("Read the next page").await?;
    let request = mock.completion.single_request();
    Ok(function_tool_output_items(&request, &call_id))
}

#[test_case(&[]; "grpc")]
#[test_case(&[Feature::CodeModeHostGrpc]; "stdio")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_pagination_completed_lines_and_utf8(
    disabled_features: &'static [Feature],
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    // The first two lines fill 14 of 16 bytes. The next line fits a fresh
    // page. The five four-byte code points necessarily cross a page boundary.
    let server = responses::start_mock_server().await;
    let (test, initial) = run_code_mode_turn_with_builder(
        &server,
        "Produce output once",
        r#"// @exec: {"max_output_tokens": 4}
text("first!\nsecond\nshort\n🦀🦀🦀🦀🦀\nend\n");
"#,
        code_mode_test_builder(disabled_features),
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    let cell_id = terminal_cell_id(&first, "Script completed");
    let mut pages = vec![body(&first)];
    for sequence in 1..=3 {
        let items = wait_page(
            &server, &test, cell_id, sequence, /*max_tokens*/ 4, /*terminate*/ false,
        )
        .await?;
        if sequence < 3 {
            assert_eq!(terminal_cell_id(&items, "Script completed"), cell_id);
        } else {
            assert_eq!(status(&items), "Script completed");
        }
        pages.push(body(&items));
    }
    assert_eq!(
        pages,
        ["first!\nsecond\n", "short\n", "🦀🦀🦀🦀", "🦀\nend\n"]
    );
    assert_eq!(pages.concat(), "first!\nsecond\nshort\n🦀🦀🦀🦀🦀\nend\n");
    let missing = wait_page(
        &server, &test, cell_id, 4, /*max_tokens*/ 4, /*terminate*/ false,
    )
    .await?;
    assert_eq!(status(&missing), "Script failed");
    assert_eq!(
        body(&missing),
        format!("Script error:\nexec cell {cell_id} not found")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_pagination_failed_keeps_its_status() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let (test, initial) = run_code_mode_turn(
        &server,
        "Run a failing script",
        r#"// @exec: {"max_output_tokens": 16}
text("a".repeat(67));
throw new Error("page failure");
"#,
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    let cell_id = terminal_cell_id(&first, "Script failed");
    assert_eq!(body(&first), "a".repeat(64));
    let last = wait_page(
        &server, &test, cell_id, 1, /*max_tokens*/ 100, /*terminate*/ false,
    )
    .await?;
    assert_eq!(status(&last), "Script failed");
    assert!(body(&last).starts_with("aaaScript error:\nError: page failure\n"));
    let missing = wait_page(
        &server, &test, cell_id, 2, /*max_tokens*/ 100, /*terminate*/ false,
    )
    .await?;
    assert_eq!(
        body(&missing),
        format!("Script error:\nexec cell {cell_id} not found")
    );
    Ok(())
}

#[test_case(false; "completion")]
#[test_case(true; "termination")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_pagination_yield_preserves_unread_output(terminate: bool) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let after_yield = if terminate {
        "await new Promise(() => {});"
    } else {
        r#"text("new\n");"#
    };
    let (test, initial) = run_code_mode_turn(
        &server,
        "Yield with unread output",
        &format!(
            r#"// @exec: {{"max_output_tokens": 2}}
text("one\ntwo\nthree\nfour\n");
yield_control();
{after_yield}
"#,
        ),
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    let cell_id = extract_running_cell_id(text_item(&first, /*index*/ 0));
    assert_eq!(body(&first), "one\ntwo\n");

    if terminate {
        let second = wait_page(
            &server, &test, &cell_id, 1, /*max_tokens*/ 2, /*terminate*/ true,
        )
        .await?;
        assert_eq!(terminal_cell_id(&second, "Script terminated"), cell_id);
        assert_eq!(body(&second), "three\n");
        let final_page = wait_page(
            &server, &test, &cell_id, 2, /*max_tokens*/ 2, /*terminate*/ false,
        )
        .await?;
        assert_eq!(status(&final_page), "Script terminated");
        assert_eq!(body(&final_page), "four\n");
    } else {
        for (sequence, expected) in [(1, "three\n"), (2, "four\n")] {
            let queued = wait_page(
                &server, &test, &cell_id, sequence, /*max_tokens*/ 2, /*terminate*/ false,
            )
            .await?;
            assert_eq!(
                extract_running_cell_id(text_item(&queued, /*index*/ 0)),
                cell_id
            );
            assert_eq!(body(&queued), expected);
        }
        let final_page = wait_page(
            &server, &test, &cell_id, 3, /*max_tokens*/ 2, /*terminate*/ false,
        )
        .await?;
        assert_eq!(status(&final_page), "Script completed");
        assert_eq!(body(&final_page), "new\n");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_pagination_preserves_whole_media() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let audio = pcm16_audio_data_url(24_000);
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        &[0, 0, 0, 255],
        1,
        1,
        image::ExtendedColorType::Rgba8,
    )?;
    let image = format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png));
    let builder = test_codex()
        .with_model_info_override("gpt-5.4", |model| {
            model.input_modalities.push(InputModality::Audio);
        })
        .with_config(|config| {
            config
                .features
                .disable(Feature::FileBackedToolAudio)
                .unwrap();
        });
    let (test, initial) = run_code_mode_turn_with_builder(
        &server,
        "Emit media across a page boundary",
        &format!(
            r#"// @exec: {{"max_output_tokens": 2}}
text("first\n"); image({image:?}); audio({audio:?}); text("last");
"#,
        ),
        builder,
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    let cell_id = terminal_cell_id(&first, "Script completed");
    assert_eq!(text_item(&first, /*index*/ 1), "first\n");
    assert_eq!(first.len(), 3);
    assert_eq!(first[2]["image_url"], image);

    let second = wait_page(
        &server, &test, cell_id, 1, /*max_tokens*/ 2, /*terminate*/ false,
    )
    .await?;
    assert_eq!(terminal_cell_id(&second, "Script completed"), cell_id);
    assert_eq!(second.len(), 2);
    assert_eq!(second[1]["audio_url"], audio);
    let third = wait_page(
        &server, &test, cell_id, 2, /*max_tokens*/ 2, /*terminate*/ false,
    )
    .await?;
    assert_eq!(status(&third), "Script completed");
    assert_eq!(body(&third), "last");
    Ok(())
}

#[test_case(&[]; "grpc")]
#[test_case(&[Feature::CodeModeHostGrpc]; "stdio")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_wait_uses_its_own_max_tokens_budget(
    disabled_features: &'static [Feature],
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let (test, initial) = run_code_mode_turn_with_builder(
        &server,
        "Start the cell",
        r#"// @exec: {"max_output_tokens": 2147483648}
text("phase 1"); yield_control(); text("0123456789".repeat(8));
"#,
        code_mode_test_builder(disabled_features),
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    assert_eq!(body(&first), "phase 1");
    let cell_id = extract_running_cell_id(text_item(&first, /*index*/ 0));
    let first_wait = wait_page(
        &server, &test, &cell_id, 1, /*max_tokens*/ 6, /*terminate*/ false,
    )
    .await?;
    assert_eq!(terminal_cell_id(&first_wait, "Script completed"), cell_id);
    assert_eq!(body(&first_wait), "012345678901234567890123");
    let last = wait_page(
        &server, &test, &cell_id, 2, /*max_tokens*/ 100, /*terminate*/ false,
    )
    .await?;
    assert_eq!(status(&last), "Script completed");
    assert_eq!(body(&first_wait) + &body(&last), "0123456789".repeat(8));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_pagination_clamps_each_page_to_the_model_policy() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let builder = test_codex().with_model_info_override("gpt-5.5", |model| {
        model.truncation_policy = TruncationPolicyConfig::bytes(64);
    });
    let (test, initial) = run_code_mode_turn_with_builder(
        &server,
        "Print beyond the model policy",
        r#"// @exec: {"max_output_tokens": 1000}
text("x".repeat(64) + "y".repeat(64) + "last");
"#,
        builder,
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    let cell_id = terminal_cell_id(&first, "Script completed");
    assert_eq!(body(&first), "x".repeat(64));
    let second = wait_page(
        &server, &test, cell_id, 1, /*max_tokens*/ 1000, /*terminate*/ false,
    )
    .await?;
    assert_eq!(terminal_cell_id(&second, "Script completed"), cell_id);
    assert_eq!(body(&second), "y".repeat(64));
    let last = wait_page(
        &server, &test, cell_id, 2, /*max_tokens*/ 1000, /*terminate*/ false,
    )
    .await?;
    assert_eq!(status(&last), "Script completed");
    assert_eq!(body(&last), "last");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cell_pagination_terminate_releases_completed_pages() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let (test, initial) = run_code_mode_turn(
        &server,
        "Complete with unread output",
        r#"// @exec: {"max_output_tokens": 4}
text("x".repeat(100));
"#,
    )
    .await?;
    let first = custom_tool_output_items(&initial.single_request(), "call-1");
    let cell_id = terminal_cell_id(&first, "Script completed");
    assert_eq!(body(&first), "x".repeat(16));
    let terminated = wait_page(
        &server, &test, cell_id, 1, /*max_tokens*/ 4, /*terminate*/ true,
    )
    .await?;
    assert_eq!(status(&terminated), "Script completed");
    assert_eq!(body(&terminated), "");
    let missing = wait_page(
        &server, &test, cell_id, 2, /*max_tokens*/ 4, /*terminate*/ false,
    )
    .await?;
    assert_eq!(
        body(&missing),
        format!("Script error:\nexec cell {cell_id} not found")
    );
    Ok(())
}
