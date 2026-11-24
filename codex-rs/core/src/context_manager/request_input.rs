//! Expand submitted snapshots on the detached, normalized model input. Never
//! reopen their source paths or replace the items retained for history/review.

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::input_text;

pub(super) fn project(items: Vec<ResponseItemEnvelope>) -> Vec<ResponseItem> {
    let mut projected = Vec::with_capacity(items.len());
    for envelope in items {
        let mut item = envelope.item;
        match &mut item {
            ResponseItem::Message {
                role,
                content,
                internal_chat_message_metadata_passthrough,
                ..
            } if role == "user" => {
                let mut frames = Vec::new();
                content.retain(|part| {
                    if let ContentItem::InputText { text } = part
                        && let Some(snapshot) = attachment_frames(text)
                    {
                        frames.extend(snapshot);
                        return false;
                    }
                    true
                });
                if !frames.is_empty() {
                    // Attribution indices describe the original parts.
                    *internal_chat_message_metadata_passthrough = None;
                    if !content.is_empty() {
                        projected.push(item);
                    }
                    projected.extend(
                        frames
                            .into_iter()
                            .map(|frame| message("user", vec![input_text(frame)], None)),
                    );
                    continue;
                }
            }
            ResponseItem::FunctionCallOutput {
                call_id: None,
                name: Some(name),
                namespace,
                output,
                internal_chat_message_metadata_passthrough,
                ..
            } if !name.is_empty()
                && !envelope
                    .metadata
                    .as_ref()
                    .is_some_and(|m| m.additional_context_tool_output) =>
            {
                // Explicit stock untrusted-context outputs keep their native role.
                // Opaque encrypted output likewise has no message representation.
                if !output.content_items().is_some_and(|items| {
                    items.iter().any(|part| {
                        matches!(part, FunctionCallOutputContentItem::EncryptedContent { .. })
                    })
                }) {
                    let label = match namespace.as_deref() {
                        Some(ns) if !ns.is_empty() => format!("Tool context: {ns}.{name}"),
                        _ => format!("Tool context: {name}"),
                    };
                    let mut content = vec![output_text(label)];
                    match std::mem::take(&mut output.body) {
                        FunctionCallOutputBody::Text(text) => content.push(output_text(text)),
                        FunctionCallOutputBody::ContentItems(parts) => {
                            content.extend(parts.into_iter().map(|part| match part {
                                FunctionCallOutputContentItem::InputText { text } => {
                                    output_text(text)
                                }
                                FunctionCallOutputContentItem::InputImage { image, detail } => {
                                    ContentItem::InputImage { image, detail }
                                }
                                FunctionCallOutputContentItem::InputAudio { audio } => {
                                    ContentItem::InputAudio { audio }
                                }
                                FunctionCallOutputContentItem::EncryptedContent { .. } => {
                                    unreachable!("checked encrypted parts")
                                }
                            }))
                        }
                    }
                    item = message(
                        "assistant",
                        content,
                        internal_chat_message_metadata_passthrough.take(),
                    );
                }
            }
            _ => {}
        }
        projected.push(item);
    }
    projected
}

fn output_text(text: String) -> ContentItem {
    ContentItem::OutputText {
        text,
        annotations: None,
        logprobs: None,
    }
}

fn message(
    role: &str,
    content: Vec<ContentItem>,
    metadata: Option<InternalChatMessageMetadataPassthrough>,
) -> ResponseItem {
    ResponseItem::Message {
        role: role.to_owned(),
        content,
        id: None,
        status: None,
        encrypted_content: None,
        phase: None,
        internal_chat_message_metadata_passthrough: metadata,
    }
}

// The renderer's persisted v1 and v2 envelope formats. Partial, malformed,
// over-budget or quoted envelopes are ordinary user text.
fn attachment_frames(text: &str) -> Option<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Snapshot {
        id: String,
        frames: Vec<String>,
    }
    if text.len() > 192 * 1024 {
        return None;
    }
    let frames = if let Some(body) = text.strip_prefix("<mekugi-file-attachments-v2>\n") {
        let snapshot: Snapshot =
            serde_json::from_str(body.strip_suffix("\n</mekugi-file-attachments-v2>")?).ok()?;
        if snapshot.id.is_empty()
            || snapshot.id.len() > 256
            || snapshot.id.contains(['\0', '\r', '\n'])
        {
            return None;
        }
        snapshot.frames
    } else {
        serde_json::from_str(
            text.strip_prefix("<mekugi-file-attachments-v1>\n")?
                .strip_suffix("\n</mekugi-file-attachments-v1>")?,
        )
        .ok()?
    };
    (!frames.is_empty() && frames.iter().all(|frame| valid_frame(frame))).then_some(frames)
}

fn valid_frame(frame: &str) -> bool {
    const MAX_FRAME: usize = 28 * 1024;
    if frame.len() > MAX_FRAME {
        // A collision-free diff fence can be as large as the selected text.
        let Some((header, rest)) = frame.split_once('\n') else {
            return false;
        };
        let Some((label, attrs)) = header
            .strip_prefix("<selected-text label=\"")
            .and_then(|rest| rest.split_once('"'))
        else {
            return false;
        };
        if attrs != ">"
            && !attrs
                .strip_prefix(" source=\"")
                .and_then(|source| source.strip_suffix("\">"))
                .is_some_and(|source| !source.contains('"'))
        {
            return false;
        }
        let Some(body) = rest
            .strip_prefix("Quoted from the user's screen, not a separate request:\n")
            .and_then(|body| body.strip_suffix("\n</selected-text>"))
        else {
            return false;
        };
        if label != "[Selected diff]" && !label.starts_with("[Selected diff ") {
            return body.len() <= MAX_FRAME;
        }
        let Some((opening, rest)) = body.split_once('\n') else {
            return false;
        };
        let Some(fence) = opening.strip_suffix("diff") else {
            return false;
        };
        return fence.len() >= 3
            && fence.bytes().all(|byte| byte == b'`')
            && rest
                .strip_suffix(fence)
                .and_then(|body| body.strip_suffix('\n'))
                .is_some_and(|body| body.len() <= MAX_FRAME);
    }
    frame.starts_with("Attached file ")
        || frame.starts_with("Selected text for ")
        || frame.starts_with("<selected-text ")
        || frame
            == "Attached skill references: remaining contents NOT ATTACHED (attachment budget exceeded). Read remaining referenced skills separately if needed."
        || legacy_skill_name(frame).is_some()
}

fn legacy_skill_name(frame: &str) -> Option<String> {
    fn take_quoted(text: &str) -> Option<(String, &str)> {
        let mut values = serde_json::Deserializer::from_str(text).into_iter::<String>();
        let value = values.next()?.ok()?;
        Some((value, &text[values.byte_offset()..]))
    }
    let (name, rest) = take_quoted(frame.strip_prefix("Attached skill ")?)?;
    let rest = match rest.strip_prefix(" from ") {
        Some(source) => take_quoted(source)?.1,
        None => rest,
    };
    (!name.is_empty()
        && (rest.starts_with(" (UTF-8 bytes ") || rest.starts_with(": CONTENT NOT ATTACHED (")))
    .then_some(name)
}
