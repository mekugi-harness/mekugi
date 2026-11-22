use super::*;

use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn request_user_input_compact_answers_round_trip_without_metadata() {
    let value = json!({"answers":[
        {"id":"q1","choice":2}, {"id":"q2","choice":1,"note":"note"},
        {"id":"q3","other":"free"}, {"id":"q4","skipped":true}
    ]});
    let response: RequestUserInputResponse = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), value);
    assert!(
        serde_json::from_value::<RequestUserInputAnswer>(
            json!({"id":"q1","choice":1,"label":"secret"})
        )
        .is_err()
    );
}

#[test]
fn request_user_input_resolves_original_option_index_and_note() {
    let question: RequestUserInputQuestion = serde_json::from_value(json!({
        "id":"q1", "question":"Which?", "options":[
            {"label":"A"}, {"label":"B","description":"impact", "preview":"diff", "recommended":true}
        ]
    })).unwrap();
    let answer: RequestUserInputAnswer =
        serde_json::from_value(json!({"id":"q1","choice":2,"note":"because"})).unwrap();
    assert_eq!(answer.selected_option(&question).unwrap().label, "B");
    assert_eq!(answer.readable_answer(&question), Some("B\nbecause".into()));
    for value in [
        json!({"id":"q1","choice":0}),
        json!({"id":"q1","choice":3}),
        json!({"id":"other","choice":1}),
        json!({"id":"q1","other":"text","choice":1}),
    ] {
        let answer: RequestUserInputAnswer = serde_json::from_value(value).unwrap();
        assert!(!answer.is_valid_for(&question));
        assert_eq!(answer.readable_answer(&question), None);
    }
}

#[test]
fn request_user_input_event_defaults_legacy_missing_is_blocking_to_true() {
    let event: RequestUserInputEvent = serde_json::from_value(json!({
        "call_id": "call-1",
        "turn_id": "turn-1",
        "questions": [{
            "id": "q1",
            "header": "Confirm",
            "question": "Continue?",
            "options": [{
                "label": "Yes",
                "description": "Continue."
            }]
        }],
        "autoResolutionMs": 60_000
    }))
    .expect("legacy request_user_input event should deserialize");

    assert_eq!(
        event,
        RequestUserInputEvent {
            call_id: "call-1".to_string(),
            turn_id: "turn-1".to_string(),
            questions: vec![RequestUserInputQuestion {
                id: "q1".to_string(),
                header: Some("Confirm".to_string()),
                question: "Continue?".to_string(),
                is_other: false,
                is_secret: false,
                options: Some(vec![RequestUserInputQuestionOption {
                    label: "Yes".to_string(),
                    description: Some("Continue.".to_string()),
                    preview: None,
                    recommended: false,
                }]),
            }],
            is_blocking: true,
            auto_resolution_ms: Some(60_000),
        }
    );
}
