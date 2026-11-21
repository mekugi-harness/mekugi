use pretty_assertions::assert_eq;

use super::HeadRetainedBuffer;
use super::MAX_RETAINED_OUTPUT_BYTES;
use super::format_discard_notice;
use super::slice_next_page;
use super::unified_exec_page_byte_budget;
use codex_utils_output_truncation::TruncationPolicy;

fn collect_all_pages(bytes: &[u8], discarded_bytes: u64, page_budget: usize) -> Vec<String> {
    let mut offset = 0usize;
    let mut reported = 0u64;
    let mut pages = Vec::new();
    loop {
        let unread = &bytes[offset..];
        let page = slice_next_page(
            unread,
            unread.len(),
            discarded_bytes,
            reported,
            page_budget,
            /*stream_complete*/ true,
        );
        offset += page.consumed_bytes;
        reported = page.reported_discarded_bytes;
        pages.push(page.text);
        if !page.has_more_output {
            break;
        }
        assert!(
            pages.len() < 10_000,
            "pagination loop did not terminate cleanly"
        );
    }
    pages
}

#[test]
fn pages_align_on_line_breaks_and_split_overlong_utf8_lines_without_losing_bytes() {
    let input = "first!\nsecond\nshort\n🦀🦀🦀🦀🦀\nend\n";
    let pages = collect_all_pages(
        input.as_bytes(),
        /*discarded_bytes*/ 0,
        /*page_budget*/ 16,
    );
    assert_eq!(
        pages,
        vec!["first!\nsecond\n", "short\n🦀🦀", "🦀🦀🦀\n", "end\n"]
    );
    assert_eq!(pages.concat(), input);
}

#[test]
fn single_overlong_line_reconstructs_exact_bytes_across_pages() {
    let input = format!("{}{}", "a".repeat(37), "🦀".repeat(15));
    let pages = collect_all_pages(
        input.as_bytes(),
        /*discarded_bytes*/ 0,
        /*page_budget*/ 13,
    );
    assert!(pages.len() > 1);
    for page in &pages {
        assert!(page.len() <= 13);
    }
    assert_eq!(pages.concat(), input);
}

#[test]
fn tiny_budget_smaller_than_utf8_char_still_makes_progress() {
    let input = "🦀🎉";
    let pages = collect_all_pages(
        input.as_bytes(),
        /*discarded_bytes*/ 0,
        /*page_budget*/ 2,
    );
    assert_eq!(pages, vec!["🦀", "🎉"]);
    assert_eq!(pages.concat(), input);
}

#[test]
fn incomplete_utf8_suffix_is_held_until_stream_completes() {
    let emoji = "🦀".as_bytes();
    let mut partial = b"hi".to_vec();
    partial.extend_from_slice(&emoji[..2]);

    let page1 = slice_next_page(
        &partial,
        partial.len(),
        /*discarded_bytes*/ 0,
        /*reported_discarded_bytes*/ 0,
        /*page_budget_bytes*/ 32,
        /*stream_complete*/ false,
    );
    assert_eq!(page1.text, "hi");
    assert_eq!(page1.consumed_bytes, 2);
    assert!(page1.has_more_output);

    partial.extend_from_slice(&emoji[2..]);
    let remaining = &partial[page1.consumed_bytes..];
    let page2 = slice_next_page(
        remaining,
        remaining.len(),
        /*discarded_bytes*/ 0,
        /*reported_discarded_bytes*/ 0,
        /*page_budget_bytes*/ 32,
        /*stream_complete*/ false,
    );
    assert_eq!(page2.text, "🦀");
    assert_eq!(page2.consumed_bytes, 4);
    assert!(!page2.has_more_output);
}

#[test]
fn windowed_slicing_matches_full_buffer_slicing() {
    let input = (0..200)
        .map(|i| format!("line-{i:04}: 🦀 payload-data\n"))
        .collect::<String>();
    let budget = 64;
    let expected = collect_all_pages(input.as_bytes(), /*discarded_bytes*/ 0, budget);

    let mut offset = 0usize;
    let mut actual = Vec::new();
    let window_cap = budget * 2 + 16;
    loop {
        let total_unread = input.len() - offset;
        let window_end = offset + total_unread.min(window_cap);
        let page = slice_next_page(
            &input.as_bytes()[offset..window_end],
            total_unread,
            /*discarded_bytes*/ 0,
            /*reported_discarded_bytes*/ 0,
            budget,
            /*stream_complete*/ true,
        );
        offset += page.consumed_bytes;
        actual.push(page.text);
        if !page.has_more_output {
            break;
        }
    }

    assert_eq!(actual, expected);
    assert_eq!(actual.concat(), input);
}

#[test]
fn head_retained_buffer_keeps_head_counts_tail_discards_and_never_middle_cuts() {
    let mut buf = HeadRetainedBuffer::<16>::default();
    buf.push_chunk(b"0123456789");
    buf.push_chunk(b"ABCDEF");
    buf.push_chunk(b"TAIL-DISCARDED");

    assert_eq!(buf.retained_bytes(), 16);
    assert_eq!(buf.discarded_bytes(), 14);
    assert_eq!(buf.total_bytes(), 30);
    assert_eq!(buf.as_bytes(), b"0123456789ABCDEF");
    assert_eq!(
        buf.to_lossy_string_with_discard_notice(),
        format!(
            "0123456789ABCDEF{}",
            format_discard_notice(14, /*leading_newline*/ true)
        )
    );
}

#[test]
fn discard_notice_appends_at_end_or_defers_when_page_is_full() {
    let mut buf = HeadRetainedBuffer::<16>::default();
    buf.push_chunk(b"0123456789ABCDEF");
    buf.push_chunk(b"overflow-bytes");

    // With budget 10, retained bytes take two pages ("0123456789", "ABCDEF")
    // and the notice doesn't fit on page 2, so it lands on page 3 without leading newline.
    let pages = collect_all_pages(
        buf.as_bytes(),
        buf.discarded_bytes(),
        /*page_budget*/ 10,
    );
    assert_eq!(
        pages,
        vec![
            "0123456789".to_string(),
            "ABCDEF".to_string(),
            format_discard_notice(14, /*leading_newline*/ false),
        ]
    );

    // With budget 200, retained bytes and notice fit together on page 1.
    let single = collect_all_pages(
        buf.as_bytes(),
        buf.discarded_bytes(),
        /*page_budget*/ 200,
    );
    assert_eq!(
        single,
        vec![format!(
            "0123456789ABCDEF{}",
            format_discard_notice(14, /*leading_newline*/ true)
        )]
    );
}

#[test]
fn default_retained_cap_is_sixteen_mib() {
    assert_eq!(MAX_RETAINED_OUTPUT_BYTES, 16 * 1024 * 1024);
    let mut buf: HeadRetainedBuffer = HeadRetainedBuffer::default();
    let chunk = vec![b'x'; 4 * 1024 * 1024];
    for _ in 0..4 {
        buf.push_chunk(&chunk);
    }
    buf.push_chunk(b"extra-tail");
    assert_eq!(buf.retained_bytes(), MAX_RETAINED_OUTPUT_BYTES);
    assert_eq!(buf.discarded_bytes(), 10);
}

#[test]
fn page_budget_takes_smaller_of_requested_and_policy_with_header_allowance() {
    assert_eq!(
        unified_exec_page_byte_budget(Some(500), TruncationPolicy::Tokens(10_000)),
        2_000
    );
    assert_eq!(
        unified_exec_page_byte_budget(Some(20_000), TruncationPolicy::Tokens(10_000)),
        40_000
    );
    // Small policy (50 tokens = 200 bytes, allowance 240 bytes): leaves room for direct header.
    assert_eq!(
        unified_exec_page_byte_budget(Some(70_000), TruncationPolicy::Tokens(50)),
        80
    );
}
