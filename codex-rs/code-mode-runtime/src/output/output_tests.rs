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
            "Code Mode output limit reached: discarded 331 bytes",
            "Script error:\nerror at capacity",
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
        NextPage::Page(
            vec![OutputItem::InputText {
                text: "🦀".into()
            }],
            true
        )
    );
    assert_eq!(
        output.next(Some(0), false),
        NextPage::Page(vec![OutputItem::InputText { text: "a".into() }], true)
    );
    assert_eq!(
        output.next(Some(0), false),
        NextPage::Page(vec![image], false)
    );
    // Taking a batch after paging must not reset or copy the retained data.
    assert_eq!(output.take().next(Some(0), true), NextPage::Empty);
}

#[test]
fn discard_notice_stays_at_gap_position_with_per_gap_counts() {
    let retention = OutputRetention::default();
    let cell = retention.register(1, Default::default());
    let mut actor = OutputCollector::Server(cell.batch());
    let entry = cell.batch();

    // Fill the cell's user budget exactly, then yield.
    actor.push(OutputItem::InputText {
        text: format!("{}\n", "A".repeat(MAX_USER_BYTES - 1)),
    });
    let OutputCollector::Server(first_batch) = actor.take() else {
        unreachable!()
    };
    entry.append(first_batch);

    // Output pushed while full is discarded.
    actor.push(OutputItem::InputText {
        text: "B1-lost\n".into(),
    });
    // Reading one 10-token page frees 40 bytes.
    assert!(matches!(
        entry.next(Some(10), /*require_output*/ false),
        NextPage::Page(_, true)
    ));
    // Newly admitted output appears after the discard notice for B1-lost.
    actor.push(OutputItem::InputText {
        text: "B2-kept\n".into(),
    });
    // Fill the remaining 32 freed bytes, overflow a second gap, free space, and admit again.
    actor.push(OutputItem::InputText {
        text: "C".repeat(32),
    });
    actor.push(OutputItem::InputText {
        text: "D1-lost-more".into(),
    });
    assert!(matches!(
        entry.next(Some(10), /*require_output*/ false),
        NextPage::Page(_, true)
    ));
    actor.push(OutputItem::InputText {
        text: "D2-kept\n".into(),
    });

    let OutputCollector::Server(second_batch) = actor.take() else {
        unreachable!()
    };
    entry.append(second_batch);

    let mut rest = Vec::new();
    while let NextPage::Page(page, more) = entry.next(Some(1_000_000), /*require_output*/ true) {
        rest.extend(page);
        if !more {
            break;
        }
    }
    let tail = &rest[rest.len() - 6..];
    let OutputItem::InputText { text: prefix_tail } = &tail[0] else {
        panic!("expected prefix tail")
    };
    assert!(prefix_tail.ends_with("A\n"));
    assert_eq!(
        &tail[1..],
        [
            "Code Mode output limit reached: discarded 8 bytes",
            "B2-kept\n",
            &"C".repeat(32),
            "Code Mode output limit reached: discarded 12 bytes",
            "D2-kept\n",
        ]
        .map(|text| OutputItem::InputText {
            text: text.to_string(),
        })
    );
}

#[test]
fn overlong_line_fills_current_page_while_fitting_line_moves_whole() {
    let drain_sizes = |input: &str| {
        let retention = OutputRetention::default();
        let cell = retention.register(1, Default::default());
        let mut actor = OutputCollector::Server(cell.batch());
        let entry = cell.batch();
        actor.push(OutputItem::InputText {
            text: input.to_string(),
        });
        let OutputCollector::Server(batch) = actor.take() else {
            unreachable!()
        };
        entry.append(batch);
        let mut sizes = Vec::new();
        let mut rebuilt = String::new();
        while let NextPage::Page(page, more) =
            entry.next(Some(10_000), /*require_output*/ true)
        {
            let mut page_bytes = 0;
            for item in page {
                if let OutputItem::InputText { text } = item {
                    page_bytes += text.len();
                    rebuilt.push_str(&text);
                }
            }
            sizes.push(page_bytes);
            if !more {
                break;
            }
        }
        assert_eq!(rebuilt, input);
        sizes
    };

    // 100 KB line exceeds 40,000-byte page budget, so it fills the first page's remainder.
    assert_eq!(
        drain_sizes(&format!("a\n{}", "q".repeat(100_000))),
        vec![40_000, 40_000, 20_002]
    );
    // A line that fits in a full 40,000-byte page moves whole to the next page.
    assert_eq!(
        drain_sizes(&format!("a\n{}", "q".repeat(39_999))),
        vec![2, 39_999]
    );
}
