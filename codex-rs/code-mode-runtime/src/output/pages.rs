use std::collections::VecDeque;

use codex_code_mode_protocol::DEFAULT_MAX_OUTPUT_TOKENS_PER_EXEC_CALL;
use codex_code_mode_protocol::FunctionCallOutputContentItem as OutputItem;
use codex_code_mode_protocol::host::MAX_FRAME_BYTES;
use codex_utils_audio::estimate_audio_token_count_without_cache;
use codex_utils_output_truncation::approx_bytes_for_tokens;

use super::discard_notice_item;
use super::discard_notice_text;

// Allow for stdio JSON escaping and item framing even at large token budgets.
const MAX_PAGE_BYTES: usize = MAX_FRAME_BYTES / 8;
const MAX_PAGE_ITEMS: usize = 4096;

#[derive(Debug)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub(super) enum BufferEntry {
    Item(MeasuredItem),
    DiscardNotice(u64),
}

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
    pub(super) items: VecDeque<BufferEntry>,
    pub(super) bytes: usize,
    text_offset: usize,
}

impl OutputBuffer {
    pub(super) fn record_discard(&mut self, lost: u64) {
        if lost == 0 {
            return;
        }
        if let Some(BufferEntry::DiscardNotice(existing)) = self.items.back_mut() {
            *existing = existing.saturating_add(lost);
        } else {
            self.items.push_back(BufferEntry::DiscardNotice(lost));
        }
    }

    pub(super) fn evict_unread(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let mut discarded = self.bytes as u64;
        for entry in self.items.drain(..) {
            if let BufferEntry::DiscardNotice(existing) = entry {
                discarded = discarded.saturating_add(existing);
            }
        }
        self.bytes = 0;
        self.text_offset = 0;
        self.record_discard(discarded);
    }

    pub(super) fn append(&mut self, mut other: Self) {
        debug_assert_eq!(other.text_offset, 0);
        if let (Some(BufferEntry::DiscardNotice(prev)), Some(BufferEntry::DiscardNotice(next))) =
            (self.items.back_mut(), other.items.front())
        {
            *prev = prev.saturating_add(*next);
            other.items.pop_front();
        }
        self.items.extend(other.items);
        self.bytes += other.bytes;
    }

    pub(super) fn has_output(&self) -> bool {
        !self.items.is_empty()
    }

    pub(super) fn next(&mut self, requested: Option<usize>) -> Vec<OutputItem> {
        let mut remaining =
            approx_bytes_for_tokens(requested.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS_PER_EXEC_CALL));
        let mut frame_remaining = MAX_PAGE_BYTES;
        let full_page_budget = remaining.min(frame_remaining);
        let mut page = Vec::new();
        while page.len() < MAX_PAGE_ITEMS
            && (page.is_empty() || remaining != 0 && frame_remaining != 0)
        {
            let Some(front) = self.items.front_mut() else {
                break;
            };
            match front {
                BufferEntry::DiscardNotice(discarded) => {
                    let text = discard_notice_text(*discarded);
                    if (text.len() > remaining || text.len() > frame_remaining) && !page.is_empty()
                    {
                        break;
                    }
                    let len = text.len();
                    self.items.pop_front();
                    page.push(OutputItem::InputText { text });
                    remaining = remaining.saturating_sub(len);
                    frame_remaining = frame_remaining.saturating_sub(len);
                }
                BufferEntry::Item(front) => {
                    let text_budget = remaining.min(frame_remaining);
                    if let OutputItem::InputText { text } = &mut front.item {
                        let rest = &text[self.text_offset..];
                        if rest.len() > text_budget {
                            let hard_end = rest.floor_char_boundary(text_budget);
                            let line_end = rest[..hard_end].rfind('\n').map_or(0, |at| at + 1);
                            let tail = &rest[line_end..];
                            let tail_exceeds_full_page = tail.len() > full_page_budget
                                && !tail[..tail.floor_char_boundary(full_page_budget)]
                                    .contains('\n');
                            let end = if tail_exceeds_full_page {
                                if page.is_empty() && hard_end == 0 {
                                    rest.chars().next().map_or(0, char::len_utf8)
                                } else {
                                    hard_end
                                }
                            } else if line_end != 0 {
                                line_end
                            } else if page.is_empty() {
                                hard_end.max(rest.chars().next().map_or(0, char::len_utf8))
                            } else {
                                0
                            };
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
                                } else if self.text_offset >= 4096
                                    && self.text_offset >= front.bytes
                                {
                                    text.drain(..std::mem::take(&mut self.text_offset));
                                    text.shrink_to_fit();
                                }
                            }
                            break;
                        }
                    }
                    if (front.charge > remaining || front.bytes > frame_remaining)
                        && !page.is_empty()
                    {
                        break;
                    }
                    let Some(BufferEntry::Item(mut front)) = self.items.pop_front() else {
                        unreachable!("front item was inspected");
                    };
                    if let OutputItem::InputText { text } = &mut front.item {
                        text.drain(..std::mem::take(&mut self.text_offset));
                    }
                    self.bytes -= front.bytes;
                    page.push(front.item);
                    remaining = remaining.saturating_sub(front.charge);
                    frame_remaining = frame_remaining.saturating_sub(front.bytes);
                }
            }
        }
        page
    }

    pub(super) fn into_items(self) -> Vec<OutputItem> {
        debug_assert_eq!(self.text_offset, 0);
        self.items
            .into_iter()
            .map(|entry| match entry {
                BufferEntry::Item(entry) => entry.item,
                BufferEntry::DiscardNotice(discarded) => discard_notice_item(discarded),
            })
            .collect()
    }
}
