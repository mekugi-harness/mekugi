//! Checks code-mode timing headers, including absent and zero host measurements.

use super::*;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;

#[test]
fn completed_timing_preserves_content_and_distinguishes_zero_from_unavailable() {
    let content = vec![
        FunctionCallOutputContentItem::InputText {
            text: "result".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "data:image/png;base64,image".to_string(),
            },
            detail: None,
        },
        FunctionCallOutputContentItem::inline_audio("data:audio/wav;base64,audio".to_string()),
    ];
    for (host_duration, expected_timing) in [
        (
            Some(Duration::from_millis(/*millis*/ 750)),
            "Wall time 1.250 seconds (code-mode 0.750 seconds; overhead 0.500 seconds)",
        ),
        (
            Some(Duration::ZERO),
            "Wall time 1.250 seconds (code-mode 0.000 seconds; overhead 1.250 seconds)",
        ),
        (
            Some(Duration::from_micros(/*micros*/ 1_251_400)),
            "Wall time 1.250 seconds (code-mode 1.251 seconds; overhead -0.001 seconds)",
        ),
        (None, "Wall time 0.8 seconds"),
    ] {
        let mut output = CodeModeToolOutput::new(
            FunctionToolOutput::from_content(content.clone(), Some(false)),
            "Script failed".to_string(),
            Duration::from_millis(/*millis*/ 750),
            host_duration,
        );
        output.set_handler_duration_ms(/*handler_duration_ms*/ 1_250);
        let mut expected_content = vec![FunctionCallOutputContentItem::InputText {
            text: format!("Script failed\n{expected_timing}\nOutput:\n"),
        }];
        expected_content.extend(content.clone());
        let expected = FunctionToolOutput::from_content(expected_content, Some(false));
        for payload in [
            ToolPayload::Custom {
                input: "text('result')".to_string(),
            },
            ToolPayload::Function {
                arguments: r#"{"cell_id":"1"}"#.to_string(),
            },
        ] {
            let response = output.to_response_item("call", &payload);
            assert_eq!(response, expected.to_response_item("call", &payload));
            // Serialization must reuse the completed value, including on retries.
            assert_eq!(output.to_response_item("call", &payload), response);
        }
        assert_eq!(output.success_for_logging(), false);
    }
}

#[test]
fn max_output_tokens_above_history_budget_preserves_spill_id_without_double_truncation() {
    use crate::tools::code_mode::clamp_code_mode_max_output_tokens;
    use crate::tools::code_mode::handle_runtime_response;
    use codex_code_mode::CellId;
    use codex_code_mode::CodeModeOutputTruncation;
    use codex_code_mode::FunctionCallOutputContentItem as CodeModeContentItem;
    use codex_code_mode::RuntimeResponse;
    use codex_models_manager::model_info::model_info_from_slug;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::openai_models::TruncationMode;
    use codex_protocol::openai_models::TruncationPolicyConfig;
    use codex_utils_output_truncation::TruncationPolicy;
    use codex_utils_output_truncation::truncate_function_output_payload;
    use codex_utils_output_truncation::with_serialization_allowance;

    let mut model_info = model_info_from_slug("gpt-5.2-codex");
    model_info.truncation_policy = TruncationPolicyConfig {
        mode: TruncationMode::Tokens,
        limit: 200,
    };
    let spill_dir = tempfile::tempdir().expect("tempdir");
    let clamped = clamp_code_mode_max_output_tokens(
        &model_info,
        Some(20_000),
        /*experimental_show_cell_overhead*/ false,
    );
    let large_text = "A".repeat(10_000);
    let response = RuntimeResponse::Result {
        cell_id: CellId::new("cell-1".to_string()),
        content_items: vec![CodeModeContentItem::InputText { text: large_text }],
        error_text: None,
        result_buffered_for: Duration::ZERO,
        code_mode_host_duration: Some(Duration::from_millis(120)),
    };
    let output = handle_runtime_response(
        &model_info,
        response,
        clamped,
        CodeModeOutputTruncation::Client,
        Duration::from_millis(120),
        /*experimental_show_cell_overhead*/ false,
        Some(spill_dir.path()),
    );
    let ResponseInputItem::CustomToolCallOutput {
        output: mut payload,
        ..
    } = output.to_response_item(
        "call-1",
        &ToolPayload::Custom {
            input: String::new(),
        },
    )
    else {
        panic!("expected CustomToolCallOutput");
    };
    let before_history = payload.clone();
    let history_policy =
        with_serialization_allowance(TruncationPolicy::from(model_info.truncation_policy));
    truncate_function_output_payload(&mut payload, history_policy, |_| 0);
    assert_eq!(payload, before_history, "history must not double-truncate");

    let FunctionCallOutputBody::ContentItems(items) = &payload.body else {
        panic!("expected content items");
    };
    let FunctionCallOutputContentItem::InputText { text } = &items[1] else {
        panic!("expected text item");
    };
    assert!(text.contains(" (id: "), "expected spill id marker in: {text}");
}
