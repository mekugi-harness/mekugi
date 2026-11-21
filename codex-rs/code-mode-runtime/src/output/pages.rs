use std::collections::VecDeque;

use codex_code_mode_protocol::DEFAULT_MAX_OUTPUT_TOKENS_PER_EXEC_CALL;
use codex_code_mode_protocol::FunctionCallOutputContentItem as OutputItem;
use codex_code_mode_protocol::host::MAX_FRAME_BYTES;
use codex_utils_audio::estimate_audio_token_count_without_cache;
use codex_utils_output_truncation::approx_bytes_for_tokens;

use super::append_discard_notice;

// Allow for stdio JSON escaping and item framing even at large token budgets.
const MAX_PAGE_BYTES: usize = MAX_FRAME_BYTES / 8;
const MAX_PAGE_ITEMS: usize = 4096;

#[derive(Debug)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub(super) struct MeasuredItem {
    pub(super) item: OutputItem,
    pub(super) bytes: usize,
    charge: usize,
}

impl MeasuredItem {
    pub(super) fn new(item: OutputItem) -> Self {
        let (bytes, charge) = match &item {
            OutputItem::InputText { text } => (text.len(), text.len()),
            OutputItem::InputImage { image_url, .. } => (image_url.len(), 0),
            OutputItem::InputAudio { audio_url } => (
                audio_url.len(),
                approx_bytes_for_tokens(estimate_audio_token_count_without_cache(audio_url)),
            ),
            OutputItem::PageBoundary { .. } => unreachable!("boundary is added after paging"),
        };
        Self {
            item,
            bytes,
            charge,
        }
    }

    pub(super) fn append_text(&mut self, suffix: &str) {
        let OutputItem::InputText { text } = &mut self.item else {
            unreachable!("only text can be extended");
        };
        text.push_str(suffix);
        self.bytes += suffix.len();
        self.charge += suffix.len();
    }
}

#[derive(Debug, Default)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub(super) struct OutputBuffer {
    pub(super) items: VecDeque<MeasuredItem>,
    pub(super) bytes: usize,
    pub(super) discarded: u64,
    text_offset: usize,
}

impl OutputBuffer {
    pub(super) fn append(&mut self, other: Self) {
        debug_assert_eq!(other.text_offset, 0);
        self.items.extend(other.items);
        self.bytes += other.bytes;
        self.discarded = self.discarded.saturating_add(other.discarded);
    }

    pub(super) fn has_output(&self) -> bool {
        !self.items.is_empty() || self.discarded != 0
    }

    pub(super) fn next(&mut self, requested: Option<usize>) -> Vec<OutputItem> {
        let mut remaining =
            approx_bytes_for_tokens(requested.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS_PER_EXEC_CALL));
        let mut frame_remaining = MAX_PAGE_BYTES;
        let mut page = Vec::new();
        while page.len() < MAX_PAGE_ITEMS
            && (page.is_empty() || remaining != 0 && frame_remaining != 0)
        {
            let Some(front) = self.items.front_mut() else {
                break;
            };
            let text_budget = remaining.min(frame_remaining);
            if let OutputItem::InputText { text } = &mut front.item {
                let rest = &text[self.text_offset..];
                if rest.len() > text_budget {
                    let end = rest.floor_char_boundary(text_budget);
                    let end = rest[..end].rfind('\n').map_or_else(
                        || {
                            if page.is_empty() {
                                end.max(rest.chars().next().map_or(0, char::len_utf8))
                            } else {
                                0
                            }
                        },
                        |at| at + 1,
                    );
                    if end != 0 {
                        page.push(OutputItem::InputText {
                            text: rest[..end].to_owned(),
                        });
                        self.text_offset += end;
                        self.bytes -= end;
                        front.bytes -= end;
                        front.charge -= end;
                        // Avoid retaining a large allocation for a tiny unread tail.
                        if front.bytes == 0 {
                            self.items.pop_front();
                            self.text_offset = 0;
                        } else if self.text_offset >= 4096 && self.text_offset >= front.bytes {
                            text.drain(..std::mem::take(&mut self.text_offset));
                            text.shrink_to_fit();
                        }
                    }
                    break;
                }
            }
            if (front.charge > remaining || front.bytes > frame_remaining) && !page.is_empty() {
                break;
            }
            let mut front = self.items.pop_front().expect("front item was inspected");
            if let OutputItem::InputText { text } = &mut front.item {
                text.drain(..std::mem::take(&mut self.text_offset));
            }
            self.bytes -= front.bytes;
            page.push(front.item);
            remaining = remaining.saturating_sub(front.charge);
            frame_remaining = frame_remaining.saturating_sub(front.bytes);
        }
        if self.items.is_empty() && page.len() < MAX_PAGE_ITEMS {
            append_discard_notice(&mut page, std::mem::take(&mut self.discarded));
        }
        page
    }

    pub(super) fn into_items(self) -> Vec<OutputItem> {
        debug_assert_eq!(self.text_offset, 0);
        let mut items = self.items.into_iter().map(|entry| entry.item).collect();
        append_discard_notice(&mut items, self.discarded);
        items
    }
}
