use super::*;
use codex_protocol::openai_models::ToolMessages;
use pretty_assertions::assert_eq;

#[test]
fn request_user_input_async_description_always_includes_sequencing_guidance() {
    let fallback = ResolvedModelMessages::bundled().request_user_input_async_description();
    assert!(fallback.contains(REQUEST_USER_INPUT_ASYNC_SEQUENCING_GUIDANCE));

    let catalog = ModelMessages {
        tools: Some(ToolMessages {
            send_user_message_async: Some(ToolMessage {
                description: Some("Catalog async wording.".to_string()),
                parameters: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        ResolvedModelMessages {
            catalog_messages: Some(&catalog),
        }
        .request_user_input_async_description(),
        format!("Catalog async wording. {REQUEST_USER_INPUT_ASYNC_SEQUENCING_GUIDANCE}"),
    );

    let empty = ModelMessages {
        tools: Some(ToolMessages {
            send_user_message_async: Some(ToolMessage {
                description: Some(String::new()),
                parameters: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        ResolvedModelMessages {
            catalog_messages: Some(&empty),
        }
        .request_user_input_async_description(),
        REQUEST_USER_INPUT_ASYNC_SEQUENCING_GUIDANCE,
    );

    let already_guided = ModelMessages {
        tools: Some(ToolMessages {
            send_user_message_async: Some(ToolMessage {
                description: Some(format!(
                    "Custom intro. {REQUEST_USER_INPUT_ASYNC_SEQUENCING_GUIDANCE}"
                )),
                parameters: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        ResolvedModelMessages {
            catalog_messages: Some(&already_guided),
        }
        .request_user_input_async_description(),
        format!("Custom intro. {REQUEST_USER_INPUT_ASYNC_SEQUENCING_GUIDANCE}"),
    );
}
