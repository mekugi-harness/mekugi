//! Projects recorded mailbox input, not a tool status or a child's own output.

use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ResponseItem;
use pulldown_cmark::Event;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use serde::Deserialize;
use serde::Serialize;

use super::ThreadItem;
use super::ThreadItemPayload;
use crate::JsonSchema;
use crate::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum AgentMailKind {
    Message,
    Reply,
}

impl ThreadItem {
    /// Renders turn-scoped plaintext consumed by the recipient. Outgoing mail
    /// accepted by the send tool has no receiving turn metadata.
    pub fn from_delivered_agent_mail(item: &ResponseItem) -> Option<(&str, Self)> {
        let ResponseItem::AgentMessage {
            id: Some(id),
            author,
            recipient,
            content,
            internal_chat_message_metadata_passthrough,
        } = item
        else {
            return None;
        };
        let turn_id = item.turn_id()?;
        let [
            AgentMessageInputContent::InputText { text },
            attachments @ ..,
        ] = content.as_slice()
        else {
            return None;
        };
        let (kind, envelope) = text.strip_prefix("Message Type: ")?.split_once('\n')?;
        let kind = match kind {
            "MESSAGE" if attachments.is_empty() => AgentMailKind::Message,
            "FINAL_ANSWER"
                if attachments
                    .iter()
                    .all(|part| matches!(part, AgentMessageInputContent::InputText { .. })) =>
            {
                AgentMailKind::Reply
            }
            _ => return None,
        };
        // Endpoint identities come from the typed mail, never its text.
        let body = envelope
            .strip_prefix("Task name: ")?
            .strip_prefix(recipient)?
            .strip_prefix("\nSender: ")?
            .strip_prefix(author)?
            .strip_prefix("\nPayload:\n")?;
        if body.trim().is_empty() {
            return None;
        }
        let (summary, details) = if kind == AgentMailKind::Reply {
            reply_sections(body).map_or((None, None), |(summary, details)| {
                (Some(summary.to_owned()), Some(details.to_owned()))
            })
        } else {
            (None, None)
        };
        let delivered_at_ms = internal_chat_message_metadata_passthrough
            .as_ref()
            .and_then(|metadata| metadata.create_time.as_ref())
            .and_then(serde_json::Number::as_f64)
            .map(|seconds| (seconds * 1000.0) as i64);
        Some((
            turn_id,
            Self {
                payload: ThreadItemPayload::AgentMail {
                    id: id.to_string(),
                    kind,
                    sender: author.clone(),
                    recipient: recipient.clone(),
                    text: body.to_owned(),
                    summary,
                    details,
                },
                started_at_ms: delivered_at_ms,
                completed_at_ms: delivered_at_ms,
            },
        ))
    }
}

/// Headings within fences or nested blocks are not reply section boundaries.
fn reply_sections(body: &str) -> Option<(&str, &str)> {
    let heading_start = body.find(|character: char| !character.is_whitespace())?;
    if heading_start != 0 && !body[..heading_start].ends_with('\n') {
        return None;
    }
    let body = &body[heading_start..];
    let (heading, rest) = body.split_once('\n')?;
    if heading.trim_end_matches('\r') != "## Summary" {
        return None;
    }
    let mut depth = 0;
    for (event, range) in Parser::new(rest).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { .. }) if depth == 0 => {
                let section = &rest[range.clone()];
                let heading = section.lines().next()?;
                let summary = rest[..range.start].trim();
                let details = if heading == "## Details" {
                    rest[range.end..].trim()
                } else {
                    rest[range.start..].trim()
                };
                return (!summary.is_empty() && !details.is_empty()).then_some((summary, details));
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    None
}
