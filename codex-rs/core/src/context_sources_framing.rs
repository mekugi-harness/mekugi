//! Byte-delimited source bodies. Headers and nested marker lookalikes are not source text.

use std::ops::Range;

const OPEN: &str = "<mekugi-source-v1 bytes=\"";
const CLOSE: &str = "\n</mekugi-source-v1>";

struct Body {
    reference: String,
    range: Range<usize>,
}

/// Projects only validated source bodies, retaining framing and failures verbatim.
/// Returns `None` when the text contains no complete source envelope.
pub(crate) fn project(
    text: &str,
    mut project_body: impl FnMut(&str, &str) -> String,
) -> Option<String> {
    let mut cursor = 0;
    let mut result = String::new();
    let mut found = false;
    while let Some(relative) = text[cursor..].find(OPEN) {
        let start = cursor + relative;
        let Some((payload, bodies, end)) = parse(&text[start..]) else {
            // A malformed outer envelope must not expose nested lookalikes as sources.
            break;
        };
        found = true;
        result.push_str(&text[cursor..start]);
        let mut output = String::new();
        let mut ranges = Vec::new();
        let mut position = 0;
        for body in bodies {
            output.push_str(&payload[position..body.range.start]);
            let replacement = project_body(&body.reference, &payload[body.range.clone()]);
            let start = output.len();
            output.push_str(&replacement);
            ranges.push(format!("{start}:{}", output.len()));
            position = body.range.end;
        }
        output.push_str(&payload[position..]);
        let ranges = ranges.join(",");
        result.push_str(&format!(
            "{OPEN}{}\" bodies=\"{ranges}\">\n{output}{CLOSE}",
            output.len()
        ));
        cursor = start + end;
    }
    if !found {
        return None;
    }
    result.push_str(&text[cursor..]);
    Some(result)
}

pub(crate) fn contains_envelope(text: &str) -> bool {
    text.find(OPEN)
        .is_some_and(|start| parse(&text[start..]).is_some())
}

fn parse(text: &str) -> Option<(&str, Vec<Body>, usize)> {
    let header_end = text.find(">\n")?;
    let header = text.get(OPEN.len()..header_end)?.strip_suffix('"')?;
    let (length, ranges) = header.split_once("\" bodies=\"")?;
    let length = length.parse::<usize>().ok()?;
    let payload_start = header_end.checked_add(2)?;
    let payload_end = payload_start.checked_add(length)?;
    let payload = text.get(payload_start..payload_end)?;
    if !text.get(payload_end..)?.starts_with(CLOSE) {
        return None;
    }
    let mut bodies = Vec::new();
    let mut previous = 0;
    for range in ranges.split(',').filter(|range| !range.is_empty()) {
        let (start, end) = range.split_once(':')?;
        let start = start.parse::<usize>().ok()?;
        let end = end.parse::<usize>().ok()?;
        if start < previous || start > end || payload.get(start..end).is_none() {
            return None;
        }
        let prefix = payload.get(previous..start)?.strip_suffix(":\n")?;
        let (_, reference) = prefix.rsplit_once("\nSource ")?;
        let reference = serde_json::from_str::<String>(reference).ok()?;
        bodies.push(Body {
            reference,
            range: start..end,
        });
        previous = end;
    }
    Some((payload, bodies, payload_end + CLOSE.len()))
}

#[cfg(test)]
#[path = "context_sources_framing_tests.rs"]
mod tests;
