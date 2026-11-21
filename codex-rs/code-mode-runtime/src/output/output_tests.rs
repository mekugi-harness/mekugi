use super::*;
use pretty_assertions::assert_eq;

#[test]
fn collector_retains_prefix_and_diagnostic_with_exact_discard_count() {
    let mut output = OutputCollector::new(CodeModeOutputTruncation::Server);
    output.push(OutputItem::InputText {
        text: "x".repeat(MAX_USER_BYTES + 327),
    });
    output.push(OutputItem::InputText {
        text: "tail".into(),
    });
    assert_eq!(
        output.collect_error("error at capacity".into()),
        "error at capacity"
    );
    let items = output.into_items();
    let OutputItem::InputText { text } = &items[0] else {
        panic!("missing prefix")
    };
    assert_eq!(text.len(), MAX_USER_BYTES);
    assert!(text.bytes().all(|byte| byte == b'x'));
    assert_eq!(
        &items[1..],
        [
            "Script error:\nerror at capacity",
            "Code Mode output limit reached: discarded 331 bytes",
        ]
        .map(|text| OutputItem::InputText { text: text.into() })
    );
}

#[test]
fn collector_restores_output_on_failed_delivery() {
    let mut output = OutputCollector::new(CodeModeOutputTruncation::Server);
    let before = OutputItem::InputText {
        text: "before".into(),
    };
    let after = OutputItem::InputText {
        text: "after".into(),
    };
    output.push(before.clone());
    assert!(!output.try_deliver(Err));
    output.push(after.clone());
    assert_eq!(output.into_items(), [before, after]);
}

#[test]
fn packed_text_keeps_print_separator() {
    let mut output = OutputCollector::new(CodeModeOutputTruncation::Server);
    for _ in 0..1025 {
        output.push(OutputItem::InputText { text: "x".into() });
    }
    let items = output.into_items();
    assert_eq!(items.len(), 1024);
    assert_eq!(
        items[1023],
        OutputItem::InputText {
            text: "x\nx".into()
        }
    );
}

#[test]
fn zero_token_pages_advance_across_utf8_and_whole_media() {
    let OutputCollector::Server(mut output) =
        OutputCollector::new(CodeModeOutputTruncation::Server)
    else {
        unreachable!()
    };
    let text = OutputItem::InputText {
        text: "🦀a".into()
    };
    let image = OutputItem::InputImage {
        image_url: "image".into(),
        detail: None,
    };
    output.push(text);
    output.push(image.clone());
    assert_eq!(
        output.next(Some(0), false),
        Some((
            vec![OutputItem::InputText {
                text: "🦀".into()
            }],
            true
        ))
    );
    assert_eq!(
        output.next(Some(0), false),
        Some((vec![OutputItem::InputText { text: "a".into() }], true))
    );
    assert_eq!(output.next(Some(0), false), Some((vec![image], false)));
    // Taking a batch after paging must not reset or copy the retained data.
    assert_eq!(output.take().next(Some(0), true), None);
}
