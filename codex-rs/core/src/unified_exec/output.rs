//! Head-bounded output retention and page slicing for unified exec streams.

use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::with_serialization_allowance;

use super::resolve_max_tokens;

pub(crate) const MAX_RETAINED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound reserved for the direct unified-exec response header so small
/// policy budgets still fit within `with_serialization_allowance(policy)`.
const DIRECT_RESPONSE_HEADER_RESERVE_BYTES: usize = 160;

/// Format the tail-discard notice appended when retained output exceeds the 16 MiB cap.
pub(crate) fn format_discard_notice(discarded_bytes: u64, leading_newline: bool) -> String {
    if leading_newline {
        format!("\n[output truncated: {discarded_bytes} bytes discarded after 16 MiB limit]")
    } else {
        format!("[output truncated: {discarded_bytes} bytes discarded after 16 MiB limit]")
    }
}

/// Compute the byte budget for a single unified-exec page from the caller's
/// `max_output_tokens` and the model's `truncation_policy`.
pub(crate) fn unified_exec_page_byte_budget(
    max_output_tokens: Option<usize>,
    truncation_policy: TruncationPolicy,
) -> usize {
    let requested_bytes =
        TruncationPolicy::Tokens(resolve_max_tokens(max_output_tokens)).byte_budget();
    let policy_bytes = truncation_policy.byte_budget();
    let allowance_bytes = with_serialization_allowance(truncation_policy).byte_budget();
    let base_budget = requested_bytes.min(policy_bytes);
    let effective = if allowance_bytes > DIRECT_RESPONSE_HEADER_RESERVE_BYTES {
        base_budget.min(allowance_bytes - DIRECT_RESPONSE_HEADER_RESERVE_BYTES)
    } else {
        base_budget
    };
    effective.max(1)
}

/// Head-bounded byte buffer that keeps the first `MAX_BYTES` bytes intact and
/// counts any overflow bytes discarded from the tail. Never middle-cuts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HeadRetainedBuffer<const MAX_BYTES: usize = MAX_RETAINED_OUTPUT_BYTES> {
    bytes: Vec<u8>,
    discarded_bytes: u64,
}

impl<const MAX_BYTES: usize> Default for HeadRetainedBuffer<MAX_BYTES> {
    fn default() -> Self {
        Self {
            bytes: Vec::new(),
            discarded_bytes: 0,
        }
    }
}

impl<const MAX_BYTES: usize> HeadRetainedBuffer<MAX_BYTES> {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let remaining = MAX_BYTES.saturating_sub(self.bytes.len());
        let keep = chunk.len().min(remaining);
        if keep > 0 {
            self.bytes.extend_from_slice(&chunk[..keep]);
        }
        let dropped = chunk.len() - keep;
        if dropped > 0 {
            self.discarded_bytes = self.discarded_bytes.saturating_add(dropped as u64);
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn discarded_bytes(&self) -> u64 {
        self.discarded_bytes
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        (self.bytes.len() as u64).saturating_add(self.discarded_bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// Render the retained head bytes with a tail discard notice when overflow occurred.
    pub(crate) fn to_lossy_string_with_discard_notice(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.discarded_bytes > 0 {
            text.push_str(&format_discard_notice(
                self.discarded_bytes,
                !text.is_empty(),
            ));
        }
        text
    }
}

/// Result of slicing a single model-visible page from retained bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PageSlice {
    pub(crate) text: String,
    /// Number of raw bytes consumed from the unread slice.
    pub(crate) consumed_bytes: usize,
    /// Updated count of discarded bytes already reported to the reader.
    pub(crate) reported_discarded_bytes: u64,
    /// Whether additional unread bytes or unreported discard notices remain.
    pub(crate) has_more_output: bool,
}

/// Return the prefix length of `bytes` excluding any potentially incomplete
/// trailing UTF-8 sequence (at most 3 bytes). Truly malformed bytes remain
/// included so they can be decoded lossily instead of stalling a live stream.
pub(crate) fn utf8_complete_prefix_len(bytes: &[u8]) -> usize {
    let mut boundary = bytes.len().saturating_sub(char::MAX.len_utf8() - 1);
    while boundary < bytes.len() {
        match std::str::from_utf8(&bytes[boundary..]) {
            Ok(valid) => return boundary + valid.len(),
            Err(error) => {
                boundary += error.valid_up_to();
                if let Some(invalid_len) = error.error_len() {
                    boundary += invalid_len;
                } else {
                    return boundary;
                }
            }
        }
    }
    bytes.len()
}

/// Find the largest index `<= limit` that does not split a valid UTF-8 scalar.
/// Runs in O(1) time by inspecting at most the 3 bytes preceding `limit`.
fn floor_utf8_boundary(bytes: &[u8], limit: usize) -> usize {
    let limit = limit.min(bytes.len());
    let mut pos = limit.saturating_sub(char::MAX.len_utf8() - 1);
    // Skip continuation bytes belonging to a scalar that started before `pos`.
    while pos < limit && (bytes[pos] & 0b1100_0000) == 0b1000_0000 {
        pos += 1;
    }
    while pos < limit {
        match std::str::from_utf8(&bytes[pos..limit]) {
            Ok(_) => return limit,
            Err(error) => {
                pos += error.valid_up_to();
                if let Some(invalid_len) = error.error_len() {
                    pos += invalid_len;
                } else {
                    return pos;
                }
            }
        }
    }
    limit
}

/// Length in bytes of the first UTF-8 scalar (or single invalid byte sequence)
/// so slicing always makes forward progress even when `budget < 4`.
fn first_utf8_unit_len(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let window = &bytes[..bytes.len().min(char::MAX.len_utf8())];
    match std::str::from_utf8(window) {
        Ok(valid) => valid.chars().next().map_or(0, char::len_utf8),
        Err(error) if error.valid_up_to() > 0 => {
            std::str::from_utf8(&window[..error.valid_up_to()])
                .ok()
                .and_then(|valid| valid.chars().next())
                .map_or(1, char::len_utf8)
        }
        Err(error) => error.error_len().unwrap_or(window.len()).max(1),
    }
}

/// Slice a single page from `window`, which represents the unread prefix of a
/// stream with `total_unread_bytes` total unread bytes remaining (`window.len() <= total_unread_bytes`).
///
/// Pages end on a line break when the next line fits in a full page; a line
/// longer than a full page starts on the current page and splits on UTF-8 char
/// boundaries. When the end of retained bytes is reached and
/// `discarded_bytes > reported_discarded_bytes`, a discard notice is appended
/// (or deferred to the next page if the current page is already full).
pub(crate) fn slice_next_page(
    window: &[u8],
    total_unread_bytes: usize,
    discarded_bytes: u64,
    reported_discarded_bytes: u64,
    page_budget_bytes: usize,
    stream_complete: bool,
) -> PageSlice {
    debug_assert!(window.len() <= total_unread_bytes);
    let budget = page_budget_bytes.max(1);
    let window_reaches_end = window.len() == total_unread_bytes;

    let available = if stream_complete && window_reaches_end {
        window
    } else {
        &window[..utf8_complete_prefix_len(window)]
    };

    let consumed_bytes = if available.is_empty() {
        0
    } else if available.len() <= budget {
        available.len()
    } else {
        let hard_end = floor_utf8_boundary(available, budget);
        let line_end = available[..hard_end]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        let tail = &available[line_end..];
        let remaining_after_line = total_unread_bytes - line_end;
        let tail_exceeds_full_page = if remaining_after_line > budget {
            let tail_hard_end = floor_utf8_boundary(tail, budget);
            !tail[..tail_hard_end].contains(&b'\n')
        } else {
            false
        };
        if tail_exceeds_full_page {
            if hard_end == 0 {
                first_utf8_unit_len(available)
            } else {
                hard_end
            }
        } else if line_end != 0 {
            line_end
        } else {
            hard_end.max(first_utf8_unit_len(available))
        }
    };

    let mut text = String::from_utf8_lossy(&available[..consumed_bytes]).into_owned();
    let remaining_after_page = total_unread_bytes - consumed_bytes;
    let mut new_reported_discarded = reported_discarded_bytes;

    if remaining_after_page == 0 && discarded_bytes > reported_discarded_bytes {
        let delta = discarded_bytes - reported_discarded_bytes;
        let notice = format_discard_notice(delta, !text.is_empty());
        if text.is_empty() || text.len().saturating_add(notice.len()) <= budget {
            text.push_str(&notice);
            new_reported_discarded = discarded_bytes;
        }
    }

    let has_more_output = remaining_after_page > 0 || new_reported_discarded < discarded_bytes;

    PageSlice {
        text,
        consumed_bytes,
        reported_discarded_bytes: new_reported_discarded,
        has_more_output,
    }
}

/// Output delivered for one unified-exec response page (or full capture for
/// internal unpaged callers).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CapturedStream {
    pub(crate) text: String,
    pub(crate) omitted_bytes: usize,
}

impl From<&str> for CapturedStream {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            omitted_bytes: 0,
        }
    }
}

impl From<String> for CapturedStream {
    fn from(text: String) -> Self {
        Self {
            text,
            omitted_bytes: 0,
        }
    }
}

impl<const N: usize> From<&HeadRetainedBuffer<N>> for CapturedStream {
    fn from(buffer: &HeadRetainedBuffer<N>) -> Self {
        Self {
            text: String::from_utf8_lossy(buffer.as_bytes()).into_owned(),
            omitted_bytes: usize::try_from(buffer.discarded_bytes()).unwrap_or(usize::MAX),
        }
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
