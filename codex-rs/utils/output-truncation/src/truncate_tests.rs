use crate::OutputBoundary;
use crate::TruncationPolicy;
use crate::approx_token_count;
use crate::approx_tokens_from_byte_count_i64;
use crate::formatted_output_at_token_boundaries;
use crate::formatted_truncate_function_output_items_with_policy;
use crate::formatted_truncate_text;
use crate::formatted_truncate_text_with_spill;
use crate::mixed_output_at_token_boundaries;
use crate::prune_truncated_output_dir;
use crate::spill_truncated_output_with_cap;
use crate::truncate_function_output_items_with_policy;
use crate::truncate_mcp_tool_result;
use crate::truncate_measured_tool_output;
use crate::truncate_text;
use crate::truncate_text_with_spill;
use crate::truncated_output_dir;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::AudioReference;
use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;

#[test]
fn truncate_mcp_tool_result_borrows_small_result() {
    let result = CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": "hello",
        })],
        structured_content: Some(serde_json::json!({"x": 1})),
        is_error: Some(false),
        meta: Some(serde_json::json!({"k": "v"})),
    };

    let truncated = truncate_mcp_tool_result(&result, /*max_bytes*/ 1024);

    assert!(matches!(truncated, std::borrow::Cow::Borrowed(_)));
    assert_eq!(truncated.as_ref(), &result);
}

#[test]
fn truncate_mcp_tool_result_compacts_large_result() {
    let max_bytes = 1024;
    let result = CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": format!("head\n{}\ntail", "\"\\\n".repeat(5_000)),
        })],
        structured_content: Some(serde_json::json!({"x": "y".repeat(5_000)})),
        is_error: Some(true),
        meta: Some(serde_json::json!({
            "private": format!("meta-head{}meta-tail", "z".repeat(5_000)),
        })),
    };

    let truncated = truncate_mcp_tool_result(&result, max_bytes).into_owned();

    let preview = truncated.content[0]["text"]
        .as_str()
        .expect("truncated result should contain a text preview");
    assert_eq!(
        truncated,
        CallToolResult {
            content: vec![serde_json::json!({
                "type": "text",
                "text": preview,
            })],
            structured_content: None,
            is_error: Some(true),
            meta: None,
        }
    );
    assert!(serde_json::to_string(&truncated).unwrap().len() <= max_bytes);
    assert!(preview.contains("head"));
    assert!(preview.contains("chars truncated"));
    assert!(preview.contains("meta-tail"));

    let truncated_again = truncate_mcp_tool_result(&truncated, max_bytes);
    assert!(matches!(truncated_again, std::borrow::Cow::Borrowed(_)));
    assert_eq!(truncated_again.as_ref(), &truncated);
}

#[test]
fn truncate_bytes_less_than_placeholder_returns_placeholder() {
    let content = "example output";

    assert_eq!(
        "Warning: truncated output (original token count: 4)\nTotal output lines: 1\n\n…13 chars truncated…t",
        formatted_truncate_text(content, TruncationPolicy::Bytes(1)),
    );
}

#[test]
fn truncate_tokens_less_than_placeholder_returns_placeholder() {
    let content = "example output";

    assert_eq!(
        "Warning: truncated output (original token count: 4)\nTotal output lines: 1\n\nex…3 tokens truncated…ut",
        formatted_truncate_text(content, TruncationPolicy::Tokens(1)),
    );
}

#[test]
fn truncate_tokens_under_limit_returns_original() {
    let content = "example output";

    assert_eq!(
        content,
        formatted_truncate_text(content, TruncationPolicy::Tokens(10)),
    );
}

#[test]
fn truncate_bytes_under_limit_returns_original() {
    let content = "example output";

    assert_eq!(
        content,
        formatted_truncate_text(content, TruncationPolicy::Bytes(20)),
    );
}

#[test]
fn truncate_tokens_over_limit_returns_truncated() {
    let content = "this is an example of a long output that should be truncated";

    assert_eq!(
        "Warning: truncated output (original token count: 15)\nTotal output lines: 1\n\nthis is an…10 tokens truncated… truncated",
        formatted_truncate_text(content, TruncationPolicy::Tokens(5)),
    );
}

#[test]
fn truncate_bytes_over_limit_returns_truncated() {
    let content = "this is an example of a long output that should be truncated";

    assert_eq!(
        "Warning: truncated output (original token count: 15)\nTotal output lines: 1\n\nthis is an exam…30 chars truncated…ld be truncated",
        formatted_truncate_text(content, TruncationPolicy::Bytes(30)),
    );
}

#[test]
fn truncate_bytes_reports_original_line_count_when_truncated() {
    let content =
        "this is an example of a long output that should be truncated\nalso some other line";

    assert_eq!(
        "Warning: truncated output (original token count: 21)\nTotal output lines: 2\n\nthis is an exam…51 chars truncated…some other line",
        formatted_truncate_text(content, TruncationPolicy::Bytes(30)),
    );
}

#[test]
fn truncate_tokens_reports_original_line_count_when_truncated() {
    let content =
        "this is an example of a long output that should be truncated\nalso some other line";

    assert_eq!(
        "Warning: truncated output (original token count: 21)\nTotal output lines: 2\n\nthis is an example o…11 tokens truncated…also some other line",
        formatted_truncate_text(content, TruncationPolicy::Tokens(10)),
    );
}

#[test]
fn truncate_middle_bytes_handles_utf8_content() {
    let s = "😀😀😀😀😀😀😀😀😀😀\nsecond line with text\n";
    let out = truncate_text(s, TruncationPolicy::Bytes(20));
    assert_eq!(out, "😀😀…21 chars truncated…with text\n");
}

#[test]
fn truncates_across_multiple_under_limit_texts_and_reports_omitted() {
    let chunk = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega.\n";
    let chunk_tokens = approx_token_count(chunk);
    assert!(chunk_tokens > 0, "chunk must consume tokens");
    let limit = chunk_tokens * 3;
    let t1 = chunk.to_string();
    let t2 = chunk.to_string();
    let t3 = chunk.repeat(10);
    let t4 = chunk.to_string();
    let t5 = chunk.to_string();

    let items = vec![
        FunctionCallOutputContentItem::InputText { text: t1.clone() },
        FunctionCallOutputContentItem::InputText { text: t2.clone() },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:mid".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputText { text: t3 },
        FunctionCallOutputContentItem::InputText { text: t4 },
        FunctionCallOutputContentItem::InputText { text: t5 },
    ];

    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(limit), |_| 0);

    assert_eq!(output.len(), 5);

    let first_text = match &output[0] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected first item: {other:?}"),
    };
    assert_eq!(first_text, &t1);

    let second_text = match &output[1] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected second item: {other:?}"),
    };
    assert_eq!(second_text, &t2);

    assert_eq!(
        output[2],
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:mid".to_string()
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        }
    );

    let fourth_text = match &output[3] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected fourth item: {other:?}"),
    };
    assert!(
        fourth_text.contains("tokens truncated"),
        "expected marker in truncated snippet: {fourth_text}"
    );

    let summary_text = match &output[4] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected summary item: {other:?}"),
    };
    assert!(summary_text.contains("omitted 2 text items"));
}

#[test]
fn truncate_function_output_items_with_policy_discards_empty_text() {
    let mut items = vec![
        FunctionCallOutputContentItem::InputText {
            text: String::new(),
        };
        16_384
    ];
    for policy in [TruncationPolicy::Bytes(0), TruncationPolicy::Tokens(1)] {
        assert_eq!(
            truncate_function_output_items_with_policy(&items, policy, |_| 0),
            Vec::new()
        );
    }

    let content = vec![
        FunctionCallOutputContentItem::InputText {
            text: "caption".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:one".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file_123".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::inline_audio("audio:one".to_string()),
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "enc_opaque".to_string(),
        },
    ];
    items.extend(content.clone());
    for policy in [TruncationPolicy::Bytes(16), TruncationPolicy::Tokens(4)] {
        assert_eq!(
            truncate_function_output_items_with_policy(&items, policy, |_| 1),
            content
        );
    }
}

#[test]
fn formatted_text_output_reuses_owned_items_under_limit() {
    let items = vec![text("alpha"), text(""), text("beta")];
    let policy = TruncationPolicy::Bytes(32);

    let borrowed = formatted_truncate_function_output_items_with_policy(&items, policy, |_| 0);
    assert_eq!(borrowed, items);

    let allocation = items.as_ptr();
    let owned = formatted_truncate_function_output_items_with_policy(items, policy, |_| 0);
    assert_eq!(owned, borrowed);
    assert_eq!(owned.as_ptr(), allocation);
}

#[test]
fn formatted_text_output_preserves_empty_leading_text_behavior() {
    let items = vec![text(""), text("abc")];

    let output = formatted_truncate_function_output_items_with_policy(
        items,
        TruncationPolicy::Bytes(0),
        |_| 0,
    );

    assert_eq!(
        output,
        vec![FunctionCallOutputContentItem::InputText {
            text: "Warning: truncated output (original token count: 1)\nTotal output lines: 1\n\n…3 chars truncated…".to_string(),
        }]
    );
}

#[test]
fn formatted_mixed_output_budgets_text_and_audio_in_order() {
    let audio = FunctionCallOutputContentItem::inline_audio("audio:one".to_owned());
    let file_image = FunctionCallOutputContentItem::InputImage {
        image: ImageReference::File {
            file_id: "file_123".to_owned(),
        },
        detail: Some(DEFAULT_IMAGE_DETAIL),
    };
    let items = vec![
        text("abcd"),
        inline_image("img:one".to_owned()),
        text("efgh"),
        audio.clone(),
        file_image.clone(),
        text("ijkl"),
        inline_image("img:two".to_owned()),
    ];

    let output = formatted_truncate_function_output_items_with_policy(
        items,
        TruncationPolicy::Bytes(12),
        |_| 1,
    );

    assert_eq!(
        output,
        vec![
            text("abcd"),
            inline_image("img:one".to_owned()),
            text("efgh"),
            audio,
            file_image,
            inline_image("img:two".to_owned()),
            text("[omitted 1 text items ...]"),
        ]
    );
}

#[test]
fn formatted_mixed_output_preserves_encrypted_content() {
    let encrypted = FunctionCallOutputContentItem::EncryptedContent {
        encrypted_content: "enc_opaque".to_owned(),
    };
    let items = vec![text("abcdefgh"), encrypted.clone()];

    let output = formatted_truncate_function_output_items_with_policy(
        items,
        TruncationPolicy::Bytes(2),
        |_| 0,
    );

    assert_eq!(output, vec![text("a…6 chars truncated…h"), encrypted]);
}

#[test]
fn truncate_function_output_items_with_policy_omits_audio_over_budget() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: "abcdefgh".to_string(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "enc_opaque".to_string(),
        },
        FunctionCallOutputContentItem::inline_audio("audio:one".to_string()),
    ];

    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Bytes(2), |_| 1);

    assert_eq!(
        output,
        vec![
            FunctionCallOutputContentItem::InputText {
                text: "a…6 chars truncated…h".to_string(),
            },
            FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "enc_opaque".to_string(),
            },
            FunctionCallOutputContentItem::InputText {
                text: "[omitted 1 audio items ...]".to_string(),
            },
        ]
    );
}

#[test]
fn repeated_file_audio_and_text_share_one_output_budget() {
    let audio = FunctionCallOutputContentItem::InputAudio {
        audio: AudioReference::File {
            file_id: "clip".into(),
        },
    };
    let items = [text("abcd"), audio.clone(), audio.clone()];
    // Text takes one token. The first reference takes two, leaving too little
    // for a second copy of the same clip under either budget.
    for policy in [TruncationPolicy::Tokens(4), TruncationPolicy::Bytes(16)] {
        assert_eq!(
            truncate_function_output_items_with_policy(&items, policy, |_| 2),
            vec![
                text("abcd"),
                audio.clone(),
                text("[omitted 1 audio items ...]")
            ],
        );
    }
}

#[test]
fn truncate_function_output_items_with_policy_charges_audio_against_byte_budget() {
    let audio = FunctionCallOutputContentItem::inline_audio("audio:one".to_string());
    let items = vec![
        audio.clone(),
        FunctionCallOutputContentItem::InputText {
            text: "abcdefgh".to_string(),
        },
    ];

    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Bytes(5), |_| 1);

    assert_eq!(
        output,
        vec![
            audio,
            FunctionCallOutputContentItem::InputText {
                text: truncate_text("abcdefgh", TruncationPolicy::Bytes(1)),
            },
        ]
    );
}

#[test]
fn formatted_text_output_merges_all_text_for_token_budget() {
    let items = vec![text("abcdefgh"), text("ijklmnop")];

    let output = formatted_truncate_function_output_items_with_policy(
        items,
        TruncationPolicy::Tokens(2),
        |_| 0,
    );

    assert_eq!(
        output,
        vec![FunctionCallOutputContentItem::InputText {
            text: "Warning: truncated output (original token count: 5)\nTotal output lines: 2\n\nabcd…3 tokens truncated…mnop".to_string(),
        }]
    );
}

#[test]
fn byte_count_conversion_clamps_non_positive_values() {
    assert_eq!(approx_tokens_from_byte_count_i64(/*bytes*/ -1), 0);
    assert_eq!(approx_tokens_from_byte_count_i64(/*bytes*/ 0), 0);
    assert_eq!(approx_tokens_from_byte_count_i64(/*bytes*/ 5), 2);
}

fn boundaries<const N: usize>(positions: [(usize, usize); N]) -> [OutputBoundary; N] {
    positions.map(|(item_index, text_offset)| OutputBoundary {
        item_index,
        text_offset,
    })
}

fn positions<const N: usize>(boundaries: [OutputBoundary; N]) -> [(usize, usize); N] {
    boundaries.map(|b| (b.item_index, b.text_offset))
}

fn text(s: &str) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputText { text: s.to_owned() }
}

fn inline_image(image_url: String) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputImage {
        image: ImageReference::Inline { image_url },
        detail: Some(DEFAULT_IMAGE_DETAIL),
    }
}

#[test]
fn boundary_formatters_normalize_input_and_remap_truncated_text() {
    let audio_cost: fn(&AudioReference) -> usize = |_| 1;
    let items = [text("left-head"), text("right-tail")];
    let max_tokens = 2;
    let mut boundary = boundaries([(0, 0), (0, 2), (1, 0), (1, 8), (2, 0)]);
    let output =
        formatted_output_at_token_boundaries(&items, max_tokens, audio_cost, &mut boundary);
    let [FunctionCallOutputContentItem::InputText { text: rendered }] = output.as_slice() else {
        panic!("expected one merged text item")
    };
    let head = rendered.find("left").expect("expected retained prefix");
    let tail = rendered
        .strip_suffix("tail")
        .expect("expected retained suffix")
        .len();
    assert_eq!(
        positions(boundary),
        [
            (0, 0),
            (0, head + 2),
            (0, tail),
            (0, tail + 2),
            (0, rendered.len())
        ]
    );

    for (middle, middle_end) in [(text("after"), 5), (inline_image("img:one".to_owned()), 0)] {
        let items = [text("🙂é boundary"), middle, text("last")];
        for formatter in [
            formatted_output_at_token_boundaries,
            mixed_output_at_token_boundaries,
        ] {
            for tokens in [0, 1, 2, 256] {
                let mut b = boundaries([(0, 1), (1, usize::MAX), (0, 8), (usize::MAX, usize::MAX)]);
                let output = formatter(&items, tokens, audio_cost, &mut b);
                assert_eq!(output, formatter(&items, tokens, audio_cost, &mut []));
                let result = positions(b);
                assert!(result.is_sorted());
                assert!(
                    result
                        .iter()
                        .all(|&(index, offset)| match output.get(index) {
                            Some(FunctionCallOutputContentItem::InputText { text }) =>
                                text.is_char_boundary(offset),
                            _ => index <= output.len() && offset == 0,
                        })
                );
                if tokens == 256 {
                    assert_eq!(result, [(0, 0), (1, middle_end), (1, middle_end), (3, 0)]);
                }
            }
        }
    }
}

#[test]
fn owned_mixed_output_moves_a_preserved_image_across_text_truncation() {
    let image_url = "data:image/png;base64,".to_owned() + &"a".repeat(4096);
    let url_ptr = image_url.as_ptr();
    let items = vec![
        text("long prefix"),
        inline_image(image_url),
        text("omitted"),
    ];
    let mut borrowed = boundaries([(0, 5), (2, 3)]);
    let mut owned = boundaries([(0, 5), (2, 3)]);
    let max_tokens = 1;
    let expected = formatted_output_at_token_boundaries(&items, max_tokens, |_| 0, &mut borrowed);
    let actual = formatted_output_at_token_boundaries(items, max_tokens, |_| 0, &mut owned);
    assert_eq!(actual, expected);
    let projected = positions(owned);
    assert_eq!(projected, positions(borrowed));
    let [(index, offset), omitted] = projected;
    let FunctionCallOutputContentItem::InputText { text: snippet } = &actual[index] else {
        panic!("expected boundary inside truncated text");
    };
    assert_eq!((index, &snippet[offset..], omitted), (0, "ix", (2, 0)));
    let FunctionCallOutputContentItem::InputImage {
        image: ImageReference::Inline { image_url },
        ..
    } = &actual[1]
    else {
        panic!("expected preserved image")
    };
    assert_eq!(image_url.as_ptr(), url_ptr);
}

#[test]
fn measured_tool_output_keeps_charges_paired_with_retained_clips() {
    let clip = FunctionCallOutputContentItem::inline_audio("clip".into());
    let text = FunctionCallOutputContentItem::InputText {
        text: "keep".into(),
    };
    let omitted = FunctionCallOutputContentItem::InputText {
        text: "[omitted 1 audio items ...]".into(),
    };
    assert_eq!(
        truncate_measured_tool_output(
            vec![
                (clip.clone(), 9),
                (text.clone(), 0),
                (clip.clone(), 2),
                (clip.clone(), 1)
            ],
            TruncationPolicy::Tokens(4)
        ),
        vec![(text, 0), (clip.clone(), 2), (clip, 1), (omitted, 0)]
    );
}

#[test]
fn formatted_truncate_text_with_spill_writes_omitted_portion_and_id() {
    let base = std::env::temp_dir().join(format!("codex-spill-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let spill_dir = truncated_output_dir(&base);

    // Non-truncated input must not create the spill directory.
    let short = "hello world";
    assert_eq!(
        formatted_truncate_text_with_spill(short, TruncationPolicy::Tokens(10), Some(&spill_dir)),
        short
    );
    assert!(!spill_dir.exists());

    // Truncated token input spills the exact omitted middle and embeds `(id: <id>)`.
    let content = format!("HEAD_PART_{}_TAIL_PART", "x".repeat(200));
    let rendered = formatted_truncate_text_with_spill(
        &content,
        TruncationPolicy::Tokens(10),
        Some(&spill_dir),
    );
    let marker_start = rendered.find(" (id: ").expect("missing spill id notice");
    let id_start = marker_start + " (id: ".len();
    let id_end = rendered[id_start..].find(')').expect("closing paren") + id_start;
    let id = &rendered[id_start..id_end];
    assert_eq!(id.len(), 8);

    let spilled = std::fs::read_to_string(spill_dir.join(id)).expect("read spilled file");
    assert!(!spilled.is_empty());
    assert!(content.contains(&spilled));

    // Byte-budget truncation also spills omitted portion.
    let byte_rendered =
        truncate_text_with_spill(&content, TruncationPolicy::Bytes(20), Some(&spill_dir));
    let b_start = byte_rendered.find(" (id: ").expect("missing byte spill id");
    let b_id_start = b_start + " (id: ".len();
    let b_id_end = byte_rendered[b_id_start..]
        .find(')')
        .expect("closing paren")
        + b_id_start;
    let b_id = &byte_rendered[b_id_start..b_id_end];
    assert_ne!(b_id, id);
    let byte_spilled = std::fs::read_to_string(spill_dir.join(b_id)).expect("read byte spilled");
    assert!(!byte_spilled.is_empty());
    assert!(content.contains(&byte_spilled));

    let _ = std::fs::remove_dir_all(&base);
}

fn write_file_with_mtime(path: &std::path::Path, content: &str, secs_since_epoch: u64) {
    std::fs::write(path, content).expect("write test spill file");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open test spill file");
    let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs_since_epoch);
    file.set_modified(mtime).expect("set test spill mtime");
}

#[test]
fn spill_truncated_output_with_cap_evicts_oldest_files_and_keeps_newest() {
    let base = std::env::temp_dir().join(format!(
        "codex-spill-evict-oldest-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let spill_dir = truncated_output_dir(&base);
    std::fs::create_dir_all(&spill_dir).expect("create spill dir");

    write_file_with_mtime(&spill_dir.join("00000001"), &"a".repeat(10), 1_000);
    write_file_with_mtime(&spill_dir.join("00000002"), &"b".repeat(10), 2_000);
    write_file_with_mtime(&spill_dir.join("00000003"), &"c".repeat(10), 3_000);

    // Cap 25 bytes: existing 30 + new 10 = 40 -> must evict 00000001 and 00000002 (leaving 20 <= 25).
    let new_id =
        spill_truncated_output_with_cap(&spill_dir, &"d".repeat(10), 25).expect("spill with cap");

    assert!(!spill_dir.join("00000001").exists());
    assert!(!spill_dir.join("00000002").exists());
    assert!(spill_dir.join("00000003").exists());
    assert_eq!(
        std::fs::read_to_string(spill_dir.join(&new_id)).expect("read new spill"),
        "d".repeat(10)
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn spill_truncated_output_with_cap_breaks_mtime_ties_deterministically() {
    let base =
        std::env::temp_dir().join(format!("codex-spill-evict-tie-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let spill_dir = truncated_output_dir(&base);
    std::fs::create_dir_all(&spill_dir).expect("create spill dir");

    write_file_with_mtime(&spill_dir.join("00000002"), &"b".repeat(10), 5_000);
    write_file_with_mtime(&spill_dir.join("00000001"), &"a".repeat(10), 5_000);

    // Cap 20 bytes: existing 20 + new 10 = 30 -> must evict "00000001" first by filename tie-break.
    let new_id =
        spill_truncated_output_with_cap(&spill_dir, &"c".repeat(10), 20).expect("spill with cap");

    assert!(!spill_dir.join("00000001").exists());
    assert!(spill_dir.join("00000002").exists());
    assert!(spill_dir.join(&new_id).exists());

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn spill_truncated_output_with_cap_keeps_new_file_even_when_exceeding_cap() {
    let base = std::env::temp_dir().join(format!(
        "codex-spill-evict-oversize-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let spill_dir = truncated_output_dir(&base);
    std::fs::create_dir_all(&spill_dir).expect("create spill dir");

    write_file_with_mtime(&spill_dir.join("000000a1"), &"a".repeat(10), 1_000);
    write_file_with_mtime(&spill_dir.join("000000a2"), &"b".repeat(10), 2_000);

    // New file is 30 bytes while cap is 15 bytes: all older files are evicted, new file stays intact.
    let new_id = spill_truncated_output_with_cap(&spill_dir, &"z".repeat(30), 15)
        .expect("spill oversize with cap");

    assert!(!spill_dir.join("000000a1").exists());
    assert!(!spill_dir.join("000000a2").exists());
    assert_eq!(
        std::fs::read_to_string(spill_dir.join(&new_id)).expect("read new oversize spill"),
        "z".repeat(30)
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn spill_truncated_output_with_cap_ignores_non_regular_entries() {
    let base = std::env::temp_dir().join(format!(
        "codex-spill-evict-nonregular-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let spill_dir = truncated_output_dir(&base);
    let nested_dir = spill_dir.join("subdir");
    std::fs::create_dir_all(&nested_dir).expect("create nested dir");
    std::fs::write(nested_dir.join("keep"), "nested").expect("write nested file");

    #[cfg(unix)]
    {
        let external = base.join("external_target");
        std::fs::write(&external, "do not delete").expect("write external file");
        std::os::unix::fs::symlink(&external, spill_dir.join("00000000")).expect("create symlink");
    }

    write_file_with_mtime(&spill_dir.join("000000b1"), &"a".repeat(15), 1_000);
    write_file_with_mtime(&spill_dir.join("notes.txt"), &"x".repeat(50), 500);

    let new_id = spill_truncated_output_with_cap(&spill_dir, &"n".repeat(15), 20)
        .expect("spill with non-regular entries");

    assert!(!spill_dir.join("000000b1").exists());
    assert!(spill_dir.join("notes.txt").exists());
    assert!(spill_dir.join(&new_id).exists());
    assert!(nested_dir.join("keep").exists());
    #[cfg(unix)]
    {
        assert!(spill_dir.join("00000000").symlink_metadata().is_ok());
        assert!(base.join("external_target").exists());
    }

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn prune_truncated_output_dir_removes_spill_dir_and_preserves_siblings() {
    let base = std::env::temp_dir().join(format!("codex-spill-prune-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let spill_dir = truncated_output_dir(&base);
    std::fs::create_dir_all(&spill_dir).expect("create spill dir");
    std::fs::write(base.join("scratch"), "keep me").expect("write sibling");
    let id = spill_truncated_output_with_cap(&spill_dir, "spilled", 1024).expect("spill");
    assert!(spill_dir.join(id).exists());

    prune_truncated_output_dir(&base).expect("prune spill dir");

    assert!(!spill_dir.exists());
    assert_eq!(
        std::fs::read_to_string(base.join("scratch")).expect("sibling preserved"),
        "keep me"
    );
    // Calling again on missing dir is a no-op.
    prune_truncated_output_dir(&base).expect("idempotent prune");

    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn spill_truncated_output_with_cap_rejects_symlinked_spill_dir() {
    let base = std::env::temp_dir().join(format!(
        "codex-spill-symlink-dir-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let outside_dir = base.join("outside");
    let session_tmp = base.join("session");
    std::fs::create_dir_all(&outside_dir).expect("create outside dir");
    std::fs::create_dir_all(&session_tmp).expect("create session dir");
    write_file_with_mtime(&outside_dir.join("00000001"), &"k".repeat(32), 1_000);

    let spill_dir = truncated_output_dir(&session_tmp);
    std::os::unix::fs::symlink(&outside_dir, &spill_dir).expect("symlink spill dir");

    let err = spill_truncated_output_with_cap(&spill_dir, "payload", 10)
        .expect_err("symlinked spill dir must be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(outside_dir.join("00000001").exists());

    let _ = std::fs::remove_dir_all(&base);
}
