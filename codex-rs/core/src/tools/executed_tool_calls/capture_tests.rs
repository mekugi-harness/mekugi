use super::*;
use codex_history::CaptureDeliveryReceipt;
use codex_protocol::agent_capture::CaptureInterval;
use codex_protocol::models::FunctionCallOutputPayload;

fn output(call_id: &str, text: String) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: call_id.to_owned(),
        name: Some("exec".to_owned()),
        output: FunctionCallOutputPayload::from_text(text),
        internal_chat_message_metadata_passthrough: None,
    })
}

fn candidate() -> CaptureDeliveryCandidate {
    CaptureDeliveryCandidate {
        rendered_text: "exact evidence\n+new line".to_owned(),
        receipt: CaptureDeliveryReceipt {
            operation_id: "progress".to_owned(),
            context_id: "context".to_owned(),
            intervals: vec![CaptureInterval {
                key: "key".to_owned(),
                change_id: "amber1".to_owned(),
                record_order: 1,
                legacy_record_key: None,
                first_chunk: 1,
                last_chunk: 2,
            }],
        },
    }
}

#[test]
fn only_complete_printed_output_carries_receipts_without_analytics() {
    for text in [
        "nothing printed".to_owned(),
        "exact evidence".to_owned(),
        candidate().rendered_text,
        serde_json::json!({"progress": candidate().rendered_text}).to_string(),
    ] {
        let recorder = ExecutedToolCalls::default();
        let cell = CellId::new("cell".to_owned());
        recorder.start_cell(&cell, "exec");
        recorder.record_capture_candidate(&cell, candidate());
        recorder.mark_terminal_capture_output(&cell, "exec");
        recorder.finish_cell_recording(&cell);
        let mut item = output("exec", text.clone());
        recorder.attach_capture_to_output(&mut item);
        let count = item
            .metadata
            .as_ref()
            .map_or(0, |m| m.capture_deliveries.len());
        assert_eq!(count, usize::from(text.contains("new line")), "{text}");
        assert!(recorder.lock_state().cells.is_empty());
    }
}

#[test]
fn yielded_output_does_not_retire_buffered_final_evidence() {
    let recorder = ExecutedToolCalls::default();
    let cell = CellId::new("cell".to_owned());
    recorder.start_cell(&cell, "exec");
    recorder.record_capture_candidate(&cell, candidate());
    recorder.finish_cell_recording(&cell);
    let mut yielded = output("exec", "no evidence yet".to_owned());
    recorder.attach_capture_to_output(&mut yielded);
    assert!(yielded.metadata.is_none());
    recorder.register_cell(&cell, "wait");
    recorder.mark_terminal_capture_output(&cell, "wait");
    let mut final_output = output("wait", candidate().rendered_text);
    recorder.attach_capture_to_output(&mut final_output);
    assert_eq!(final_output.metadata.unwrap().capture_deliveries.len(), 1);
}

#[test]
fn notification_matching_and_analytics_refresh_preserve_candidates() {
    let recorder = ExecutedToolCalls::default();
    let cell = CellId::new("cell".to_owned());
    recorder.start_cell(&cell, "exec");
    recorder.record_capture_candidate(&cell, candidate());
    let mut features = Features::with_defaults();
    features.enable(Feature::ExecutedToolCallMetadata);
    recorder.refresh(&features);
    features.disable(Feature::ExecutedToolCallMetadata);
    recorder.refresh(&features);
    let notification = output("notify", candidate().rendered_text);
    assert_eq!(
        recorder
            .capture_receipts_for_cell(&cell, &notification.item)
            .len(),
        1
    );
    assert_eq!(
        recorder.lock_state().cells[&cell].capture_candidates.len(),
        1
    );
}

#[test]
fn oversized_candidate_never_acknowledges_evidence() {
    let recorder = ExecutedToolCalls::default();
    let cell = CellId::new("cell".to_owned());
    recorder.start_cell(&cell, "exec");
    let mut candidate = candidate();
    candidate.rendered_text = "x".repeat(MAX_PENDING_CAPTURE_BYTES + 1);
    recorder.record_capture_candidate(&cell, candidate);
    assert!(
        recorder.lock_state().cells[&cell]
            .capture_candidates
            .is_empty()
    );
}
