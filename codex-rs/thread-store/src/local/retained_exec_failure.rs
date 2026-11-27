//! Restores older exec failures from the host's original paired response records.

use std::collections::BTreeSet;

use codex_app_server_protocol::ThreadHistoryChangeSet;
use codex_app_server_protocol::ThreadHistoryItemChange;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadItemPayload;
use codex_protocol::items::FunctionCallOutputItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_rollout::RolloutItem;

#[derive(Default)]
pub(super) struct ExecFailureReplay {
    turn: Option<String>,
    calls: BTreeSet<String>,
    changed: bool,
}

impl ExecFailureReplay {
    pub(super) fn resume(
        turn: Option<String>,
        pending: Option<String>,
    ) -> serde_json::Result<Self> {
        let mut replay = Self {
            turn,
            ..Self::default()
        };
        if let (Some(_), Some(pending)) = (&replay.turn, pending) {
            replay.calls = serde_json::from_str(&pending)?;
        }
        Ok(replay)
    }

    pub(super) fn observe(&mut self, item: &RolloutItem, changes: &mut ThreadHistoryChangeSet) {
        let pending = self.calls.len();
        match item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                self.turn = Some(event.turn_id.clone());
                self.calls.clear();
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event))
                if self.turn.as_deref() == Some(&event.turn_id) =>
            {
                self.turn = None;
                self.calls.clear();
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event))
                if event.turn_id.is_none() || event.turn_id == self.turn =>
            {
                self.turn = None;
                self.calls.clear();
            }
            RolloutItem::EventMsg(EventMsg::ItemCompleted(event))
                if self.turn.as_deref() == Some(&event.turn_id) =>
            {
                // New rollouts already carry the canonical full diagnostic.
                if let TurnItem::FunctionCallOutput(output) = &event.item
                    && let Some(call_id) = output.code_mode_failure_call_id()
                {
                    self.calls.remove(call_id);
                }
            }
            RolloutItem::ResponseItem(response) if let Some(turn) = &self.turn => {
                if let Some(failure) = legacy_exec_failure(&mut self.calls, &response.item) {
                    changes.changed_items.push(ThreadHistoryItemChange {
                        turn_id: turn.clone(),
                        item: ThreadItem::untimed(ThreadItemPayload::from(
                            TurnItem::FunctionCallOutput(failure),
                        )),
                    });
                }
            }
            _ => {}
        }
        self.changed |= self.calls.len() != pending;
    }

    pub(super) fn take_checkpoint(&mut self) -> serde_json::Result<Option<Option<String>>> {
        if !std::mem::take(&mut self.changed) {
            return Ok(None);
        }
        if self.calls.is_empty() {
            return Ok(Some(None));
        }
        serde_json::to_string(&self.calls).map(|pending| Some(Some(pending)))
    }
}

// Source: mekugi-tui/backends/codex/binding/execution_output.go:22:80,162:180@cc041cd049.
pub(super) fn legacy_exec_failure(
    calls: &mut BTreeSet<String>,
    item: &ResponseItem,
) -> Option<FunctionCallOutputItem> {
    let (call_id, output) = match item {
        ResponseItem::CustomToolCall {
            call_id,
            name,
            namespace,
            ..
        } if name == "exec" && matches!(namespace.as_deref(), None | Some("" | "functions")) => {
            calls.insert(call_id.clone());
            return None;
        }
        ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } if calls.remove(call_id) => (call_id, output),
        _ => return None,
    };
    let (header, detail) = match &output.body {
        FunctionCallOutputBody::Text(text) => (text.as_str(), None),
        FunctionCallOutputBody::ContentItems(items) => {
            let mut text = items.iter().filter_map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } => Some(text.as_str()),
                _ => None,
            });
            (text.next()?, text.next_back())
        }
    };
    let mut lines = header.lines();
    let mut status = lines.next()?;
    if let Some(label) = status.strip_prefix("[call c") {
        let number = label.strip_suffix(']')?;
        if number.is_empty()
            || number.starts_with('0')
            || !number.bytes().all(|digit| digit.is_ascii_digit())
        {
            return None;
        }
        status = lines.next()?;
    }
    if status != "Script failed"
        && !status
            .strip_prefix("Script failed (more output: wait with cell ID ")
            .and_then(|cell| cell.strip_suffix(')'))
            .is_some_and(|cell| !cell.is_empty() && !cell.contains(['(', ')', ' ', '\t']))
    {
        return None;
    }
    let mut timing = lines.next()?.strip_prefix("Wall time ")?;
    if let Some((summary, breakdown)) = timing.split_once(" (code-mode ") {
        let (host, overhead) = breakdown.split_once(" seconds; overhead ")?;
        host.parse::<f64>().ok()?;
        overhead.strip_suffix(" seconds)")?.parse::<f64>().ok()?;
        timing = summary;
    }
    timing.strip_suffix(" seconds")?.parse::<f64>().ok()?;
    if lines.next()? != "Output:" {
        return None;
    }
    let diagnostic = detail
        .and_then(|part| part.strip_prefix("Script error:\n"))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_default();
    Some(FunctionCallOutputItem::code_mode_failure(
        call_id,
        diagnostic.to_string(),
    ))
}
