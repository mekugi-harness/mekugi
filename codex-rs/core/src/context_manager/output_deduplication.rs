//! Fresh, forward-only model-input projection. Only earlier literal text is a reuse base.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Range;

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::InterAgentCommunication;

const MIN_RUN: usize = 192;
const MIN_SAVING: usize = 128;
const WINDOW_BYTES: usize = 256 << 10;
const DIFF_BASES: usize = 4;
const FRAGMENTED_PARTIAL_REFERENCES: usize = 2;
const MAX_DIFF_CLARITY_OVERHEAD: usize = 2048;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Savings {
    pub bytes: u64,
    pub tokens: u64,
}

/// Mutates only the detached request copy, never retained history or its annotations.
pub(super) fn project(items: &mut [ResponseItemEnvelope]) -> Savings {
    let mut window = Window::default();
    let mut call = 0;
    let mut original_bytes = 0usize;
    let mut projected_bytes = 0usize;
    for envelope in items {
        original_bytes += text_bytes(&envelope.item);
        let mut project_sources = |text: &mut String| {
            if let Some(projected) =
                crate::context_sources::project_source_bodies(text, |reference, body| {
                    window.project(body, Some(Anchor::source(reference)), 0)
                })
            {
                *text = projected;
            }
        };
        match &mut envelope.item {
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                let texts: Vec<&mut String> = match &mut output.body {
                    FunctionCallOutputBody::Text(text) => vec![text],
                    FunctionCallOutputBody::ContentItems(parts) => parts
                        .iter_mut()
                        .filter_map(|part| match part {
                            FunctionCallOutputContentItem::InputText { text } => Some(text),
                            _ => None,
                        })
                        .collect(),
                };
                let count = texts.len();
                let mut line_offset = 0;
                let number = call + 1;
                let label = format!("[call c{number}]\n");
                let reference_size = format!("[identical to call c{number} above]").len();
                // Labels cannot depend on a future duplicate: appending input must
                // never change the already-projected request prefix.
                let labeled = texts.iter().any(|text| {
                    text.len() >= MIN_SAVING + label.len() + reference_size
                        && !crate::context_sources::contains_source_envelope(text)
                });
                if labeled {
                    call = number;
                }
                let mut overhead = if labeled { label.len() } else { 0 };
                for (index, text) in texts.into_iter().enumerate() {
                    let lines = text.split_inclusive('\n').count();
                    let projected =
                        crate::context_sources::project_source_bodies(text, |reference, body| {
                            let projected =
                                window.project(body, Some(Anchor::source(reference)), overhead);
                            if projected.len() < body.len() {
                                overhead = 0;
                            }
                            projected
                        });
                    if let Some(projected) = projected {
                        *text = projected;
                    } else {
                        let projected = window.project(
                            text,
                            labeled.then(|| Anchor::call(number, line_offset, count == 1)),
                            overhead,
                        );
                        if projected.len() < text.len() {
                            overhead = 0;
                        }
                        *text = projected;
                    }
                    if labeled && index == 0 {
                        text.insert_str(0, &label);
                    }
                    line_offset += lines;
                }
            }
            ResponseItem::AgentMessage { content, .. } => {
                for part in content {
                    if let AgentMessageInputContent::InputText { text } = part {
                        project_sources(text);
                    }
                }
            }
            ResponseItem::Message { role, content, .. } if role == "assistant" => {
                // Legacy inter-agent messages wrap the acquired text in JSON.
                // Ordinary assistant prose and every user-typed message are excluded.
                if let Some(mut communication) =
                    InterAgentCommunication::from_message_content(content)
                    && communication.encrypted_content.is_none()
                {
                    let original = communication.content.clone();
                    project_sources(&mut communication.content);
                    if communication.content != original {
                        let encoded = serde_json::to_string(&communication)
                            .expect("inter-agent communication is serializable");
                        match &mut content[0] {
                            ContentItem::InputText { text }
                            | ContentItem::OutputText { text, .. } => *text = encoded,
                            _ => unreachable!("inter-agent messages contain text"),
                        }
                    }
                }
            }
            _ => {}
        }
        projected_bytes += text_bytes(&envelope.item);
    }
    let bytes = original_bytes.saturating_sub(projected_bytes) as u64;
    Savings {
        bytes,
        tokens: bytes / 4,
    }
}

fn text_bytes(item: &ResponseItem) -> usize {
    match item {
        ResponseItem::FunctionCallOutput { output, .. }
        | ResponseItem::CustomToolCallOutput { output, .. } => match &output.body {
            FunctionCallOutputBody::Text(text) => text.len(),
            FunctionCallOutputBody::ContentItems(parts) => parts
                .iter()
                .map(|part| match part {
                    FunctionCallOutputContentItem::InputText { text } => text.len(),
                    _ => 0,
                })
                .sum(),
        },
        ResponseItem::AgentMessage { content, .. } => content
            .iter()
            .map(|part| match part {
                AgentMessageInputContent::InputText { text } => text.len(),
                _ => 0,
            })
            .sum(),
        ResponseItem::Message { content, .. } => content
            .iter()
            .map(|part| match part {
                ContentItem::InputText { text } | ContentItem::OutputText { text, .. } => {
                    text.len()
                }
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

#[derive(Clone)]
struct Anchor {
    reference: String,
    selected_lines: Vec<(usize, usize)>,
    line_offset: usize,
    whole: bool,
    within_output: bool,
}

impl Anchor {
    fn call(number: usize, line_offset: usize, whole: bool) -> Self {
        Self {
            reference: format!("call c{number}"),
            selected_lines: Vec::new(),
            line_offset,
            whole,
            within_output: true,
        }
    }

    fn source(reference: &str) -> Self {
        let selected_lines = reference
            .rsplit_once("#L")
            .and_then(|(_, ranges)| {
                ranges
                    .split(',')
                    .map(|range| {
                        let (start, end) = range.split_once('-').unwrap_or((range, range));
                        let start = start.trim_start_matches('L').parse().ok()?;
                        let end = end.trim_start_matches('L').parse().ok()?;
                        (start > 0 && end >= start).then_some((start, end))
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .unwrap_or_default();
        Self {
            reference: reference.to_owned(),
            selected_lines,
            line_offset: 0,
            whole: true,
            within_output: false,
        }
    }

    fn reference(&self, start: usize, end: usize, whole: bool) -> String {
        if whole && self.whole {
            return self.reference.clone();
        }
        let start = self.line_offset + start;
        let end = self.line_offset + end;
        if self.selected_lines.is_empty() {
            return format!("{}#L{}-L{end}", self.reference, start);
        }
        let mut cursor = 1;
        let mut ranges = Vec::new();
        for &(first, last) in &self.selected_lines {
            let next = cursor + last - first + 1;
            let a = start.max(cursor);
            let b = end.min(next - 1);
            if a <= b {
                let a = first + a - cursor;
                let b = first + b - cursor;
                ranges.push(format!("L{a}-L{b}"));
            }
            cursor = next;
        }
        let reference = self.reference.rsplit_once("#L").unwrap().0;
        format!("{reference}#{}", ranges.join(","))
    }

    fn suffix(&self, skipped_lines: usize) -> Self {
        Self {
            line_offset: self.line_offset + skipped_lines,
            whole: false,
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct Key {
    hash: u64,
    bytes: usize,
}

impl Key {
    fn new(text: &str) -> Self {
        let mut hash = DefaultHasher::new();
        text.hash(&mut hash);
        Self {
            hash: hash.finish(),
            bytes: text.len(),
        }
    }
}

struct Body {
    text: String,
    anchor: Anchor,
    lines: Vec<usize>,
    seeds: Vec<(Key, usize)>,
    complete: bool,
}

#[derive(Default)]
struct Window {
    bodies: BTreeMap<usize, Body>,
    seeds: HashMap<Key, VecDeque<(usize, usize)>>,
    wholes: HashMap<Key, VecDeque<usize>>,
    next_id: usize,
    bytes: usize,
}

#[derive(Clone, Copy)]
struct Line {
    start: usize,
    literal_offset: usize,
    number: usize,
}

#[derive(Default)]
struct LocalRuns {
    pending: VecDeque<(Key, Line)>,
    visible: VecDeque<(Key, Line)>,
    seeds: HashMap<Key, VecDeque<Line>>,
}

impl LocalRuns {
    fn advance(&mut self, cursor: usize, literal_bytes: usize) {
        let oldest = literal_bytes.saturating_sub(WINDOW_BYTES);
        while self
            .visible
            .front()
            .is_some_and(|(_, line)| line.literal_offset < oldest)
        {
            let (key, _) = self.visible.pop_front().unwrap();
            let positions = self.seeds.get_mut(&key).unwrap();
            positions.pop_front();
            if positions.is_empty() {
                self.seeds.remove(&key);
            }
        }
        // Seed endpoints are nondecreasing. Publish a seed only when every
        // line it covers has passed the cursor without being replaced.
        while self
            .pending
            .front()
            .is_some_and(|(key, line)| line.start + key.bytes <= cursor)
        {
            let (key, line) = self.pending.pop_front().unwrap();
            if line.literal_offset >= oldest {
                self.visible.push_back((key, line));
                self.seeds.entry(key).or_default().push_back(line);
            }
        }
    }
}

struct Partial {
    text: String,
    literals: Vec<Range<usize>>,
    references: usize,
}

impl Window {
    fn project(&mut self, text: &str, anchor: Option<Anchor>, overhead: usize) -> String {
        let mut projected = text.to_owned();
        let mut literals = vec![0..text.len()];
        if let Some(ids) = self.wholes.get(&Key::new(text)) {
            for id in ids {
                let base = &self.bodies[id];
                if base.text == text {
                    let reference = base.anchor.reference(1, base.lines.len(), true);
                    let candidate = format!("[identical to {reference} above]");
                    if saves(text, &candidate, overhead) {
                        return candidate;
                    }
                }
            }
        }

        let partial = self.partial(text, anchor.as_ref().filter(|anchor| anchor.within_output));
        let partial_references = if saves(text, &partial.text, overhead) {
            projected = partial.text;
            literals = partial.literals;
            partial.references
        } else {
            0
        };
        let mut best_diff: Option<String> = None;
        for base in self
            .bodies
            .values()
            .rev()
            .filter(|base| base.complete && text.len() <= WINDOW_BYTES)
            .take(DIFF_BASES)
        {
            let diff = similar::TextDiff::configure()
                .algorithm(similar::Algorithm::Patience)
                .diff_lines(base.text.as_str(), text);
            let reused: usize = diff
                .ops()
                .iter()
                .filter(|op| op.tag() == similar::DiffTag::Equal)
                .map(|op| {
                    let range = op.old_range();
                    let start = base
                        .lines
                        .get(range.start)
                        .copied()
                        .unwrap_or(base.text.len());
                    let end = base
                        .lines
                        .get(range.end)
                        .copied()
                        .unwrap_or(base.text.len());
                    end - start
                })
                .sum();
            if reused < MIN_RUN {
                continue;
            }
            let reference = base.anchor.reference(1, base.lines.len(), false);
            let changes = diff.unified_diff().context_radius(0).to_string();
            let candidate = format!(
                "[identical to {reference} above]\nApply this diff to the referenced lines:\n```diff\n{changes}```"
            );
            if saves(text, &candidate, overhead)
                && best_diff
                    .as_ref()
                    .is_none_or(|best| candidate.len() < best.len())
            {
                best_diff = Some(candidate);
            }
        }
        if let Some(diff) = best_diff
            && prefer_diff(text.len(), projected.len(), partial_references, diff.len())
        {
            projected = diff;
            literals.clear();
        }
        let Some(anchor) = anchor else {
            return projected;
        };
        for span in literals {
            if span.len() < MIN_SAVING {
                continue;
            }
            let base_anchor = if span.start == 0 && span.end == text.len() {
                anchor.clone()
            } else {
                anchor.suffix(
                    text[..span.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count(),
                )
            };
            let complete = span.start == 0 && span.end == text.len();
            self.insert(&text[span], base_anchor, complete);
        }
        projected
    }

    fn partial(&self, text: &str, local_anchor: Option<&Anchor>) -> Partial {
        let mut result = String::new();
        let mut literals: Vec<Range<usize>> = Vec::new();
        let mut references = 0;
        let mut cursor = 0;
        let mut literal_start = 0;
        let mut literal_bytes = 0;
        let mut line_number = 1;
        let mut local = local_anchor.map(|_| LocalRuns::default());
        while cursor < text.len() {
            let remaining = &text[cursor..];
            let key = seed(remaining);
            if let Some(local) = &mut local {
                local.advance(cursor, literal_bytes);
            }
            let matched = key.and_then(|key| {
                self.seeds
                    .get(&key)
                    .and_then(|positions| {
                        positions.iter().find_map(|(id, line)| {
                            let base = &self.bodies[id];
                            run_reference(
                                remaining,
                                &base.text[base.lines[*line]..],
                                &base.anchor,
                                line + 1,
                            )
                        })
                    })
                    .or_else(|| {
                        let anchor = local_anchor?;
                        local.as_ref()?.seeds.get(&key)?.iter().find_map(|line| {
                            let end = if line.start >= literal_start {
                                cursor
                            } else {
                                literals[literals.partition_point(|span| span.end <= line.start)]
                                    .end
                            };
                            run_reference(remaining, &text[line.start..end], anchor, line.number)
                        })
                    })
            });
            if let Some((bytes, lines, replacement)) = matched {
                result.push_str(&text[literal_start..cursor]);
                if literal_start < cursor {
                    literals.push(literal_start..cursor);
                }
                result.push_str(&replacement);
                references += 1;
                cursor += bytes;
                literal_start = cursor;
                line_number += lines;
                if let Some(local) = &mut local {
                    local.pending.clear();
                }
            } else {
                if let (Some(local), Some(key)) = (&mut local, key) {
                    local.pending.push_back((
                        key,
                        Line {
                            start: cursor,
                            literal_offset: literal_bytes,
                            number: line_number,
                        },
                    ));
                }
                let line_bytes = remaining.find('\n').map_or(remaining.len(), |end| end + 1);
                cursor += line_bytes;
                literal_bytes += line_bytes;
                line_number += 1;
            }
        }
        result.push_str(&text[literal_start..]);
        if literal_start < text.len() {
            literals.push(literal_start..text.len());
        }
        Partial {
            text: result,
            literals,
            references,
        }
    }

    fn insert(&mut self, text: &str, mut anchor: Anchor, mut complete: bool) {
        // Even the shortest reference must pay for itself. In particular, a long
        // source identity must not index thousands of non-saving repeated seeds.
        let identity = anchor
            .reference
            .rsplit_once("#L")
            .map_or(anchor.reference.as_str(), |(identity, _)| identity);
        let minimum_reference = "[identical to  above]".len() + identity.len();
        if text.len() < minimum_reference + MIN_SAVING {
            return;
        }
        let text = if text.len() > WINDOW_BYTES {
            let skip = text.len() - WINDOW_BYTES;
            let Some(end) = text.as_bytes()[skip..]
                .iter()
                .position(|byte| *byte == b'\n')
            else {
                return;
            };
            let skip = skip + end + 1;
            anchor = anchor.suffix(text[..skip].bytes().filter(|byte| *byte == b'\n').count());
            complete = false;
            &text[skip..]
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        while self.bytes + text.len() > WINDOW_BYTES {
            let Some((id, body)) = self.bodies.pop_first() else {
                break;
            };
            self.bytes -= body.text.len();
            for (key, line) in body.seeds {
                let positions = self.seeds.get_mut(&key).unwrap();
                assert_eq!(positions.pop_front(), Some((id, line)));
                if positions.is_empty() {
                    self.seeds.remove(&key);
                }
            }
            let key = Key::new(&body.text);
            let ids = self.wholes.get_mut(&key).unwrap();
            assert_eq!(ids.pop_front(), Some(id));
            if ids.is_empty() {
                self.wholes.remove(&key);
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut offset = 0;
        let mut lines = Vec::new();
        let mut seeds = Vec::new();
        for line in text.split_inclusive('\n') {
            if let Some(key) = seed(&text[offset..]) {
                self.seeds
                    .entry(key)
                    .or_default()
                    .push_back((id, lines.len()));
                seeds.push((key, lines.len()));
            }
            lines.push(offset);
            offset += line.len();
        }
        self.wholes.entry(Key::new(text)).or_default().push_back(id);
        self.bytes += text.len();
        self.bodies.insert(
            id,
            Body {
                text: text.to_owned(),
                anchor,
                lines,
                seeds,
                complete,
            },
        );
    }
}

fn run_reference(
    remaining: &str,
    literal: &str,
    anchor: &Anchor,
    start: usize,
) -> Option<(usize, usize, String)> {
    let (mut bytes, mut lines) = (0, 0);
    for (a, b) in remaining
        .split_inclusive('\n')
        .zip(literal.split_inclusive('\n'))
    {
        if a != b {
            break;
        }
        bytes += a.len();
        lines += 1;
    }
    if bytes < MIN_RUN {
        return None;
    }
    let reference = anchor.reference(start, start + lines - 1, false);
    let mut replacement = format!("[identical to {reference} above]");
    if remaining.as_bytes()[bytes - 1] == b'\n' {
        replacement.push('\n');
    }
    saves(&remaining[..bytes], &replacement, 0).then_some((bytes, lines, replacement))
}

fn seed(text: &str) -> Option<Key> {
    let bytes = text.as_bytes();
    if bytes.len() < MIN_RUN {
        return None;
    }
    let end = if bytes[MIN_RUN - 1] == b'\n' {
        MIN_RUN
    } else {
        let end = bytes.len().min(WINDOW_BYTES);
        bytes[MIN_RUN..end]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |end| MIN_RUN + end + 1)
    };
    (end <= WINDOW_BYTES).then(|| Key::new(&text[..end]))
}

fn saves(original: &str, replacement: &str, overhead: usize) -> bool {
    original.len().saturating_sub(replacement.len() + overhead) >= MIN_SAVING
}

fn prefer_diff(
    original_len: usize,
    partial_len: usize,
    partial_references: usize,
    diff_len: usize,
) -> bool {
    if diff_len < partial_len {
        return true;
    }
    if partial_references < FRAGMENTED_PARTIAL_REFERENCES {
        return false;
    }
    let partial_savings = original_len.saturating_sub(partial_len);
    let diff_savings = original_len.saturating_sub(diff_len);
    diff_len - partial_len <= MAX_DIFF_CLARITY_OVERHEAD && diff_savings * 4 >= partial_savings * 3
}

#[cfg(test)]
#[path = "output_deduplication_tests.rs"]
mod tests;
