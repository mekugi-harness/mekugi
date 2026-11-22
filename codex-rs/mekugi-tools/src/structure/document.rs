use std::collections::HashSet;
use std::sync::OnceLock;

use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Parser as MdParser;
use pulldown_cmark::Tag;
use regex::Regex;
use tree_sitter::Node;

use super::Source;
use super::Token;
use super::named_children;
use super::parse_tree;

/// Decode a JavaScript single- or double-quoted string literal (matching `decodeJSString`).
pub(crate) struct DecodedJsString {
    pub display: String,
    /// Pre-encoded JSON string literal (including surrounding quotes) preserving lone
    /// UTF-16 surrogates as `\ud800`..`\udfff` without double-escaping when present.
    pub raw_json: Option<String>,
}

fn push_json_escaped_char(out: &mut String, ch: char) {
    match ch {
        '"' => out.push_str(r#"\""#),
        '\\' => out.push_str(r"\\"),
        '\u{08}' => out.push_str(r"\b"),
        '\u{0c}' => out.push_str(r"\f"),
        '\n' => out.push_str(r"\n"),
        '\r' => out.push_str(r"\r"),
        '\t' => out.push_str(r"\t"),
        c if (c as u32) < 0x20 => {
            out.push_str(&format!(r"\u{:04x}", c as u32));
        }
        c => out.push(c),
    }
}

pub(crate) fn decode_js_string_parts(literal: &str) -> Option<DecodedJsString> {
    let bytes = literal.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let quote = bytes[0];
    if (quote != b'\'' && quote != b'"') || bytes[bytes.len() - 1] != quote {
        return None;
    }

    let mut units: Vec<u16> = Vec::new();
    let mut body = &literal[1..literal.len() - 1];
    while let Some(ch) = body.chars().next() {
        body = &body[ch.len_utf8()..];
        if ch as u32 == u32::from(quote) || ch == '\n' || ch == '\r' {
            return None;
        }
        if ch != '\\' {
            let mut buf = [0u16; 2];
            units.extend_from_slice(ch.encode_utf16(&mut buf));
            continue;
        }
        let esc = body.chars().next()?;
        body = &body[esc.len_utf8()..];
        let decoded_code: u32 = match esc {
            '\r' => {
                if let Some(rest) = body.strip_prefix('\n') {
                    body = rest;
                }
                continue;
            }
            '\n' | '\u{2028}' | '\u{2029}' => continue,
            'b' => 0x08,
            'f' => 0x0c,
            'n' => 0x0a,
            'r' => 0x0d,
            't' => 0x09,
            'v' => 0x0b,
            '0' => {
                if body.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                0x00
            }
            '1'..='9' => return None,
            'x' | 'u' => {
                let braced = esc == 'u' && body.starts_with('{');
                let width = if braced {
                    body = &body[1..];
                    let pos = body.find('}')?;
                    if pos < 1 {
                        return None;
                    }
                    pos
                } else if esc == 'x' {
                    2
                } else {
                    4
                };
                if body.len() < width {
                    return None;
                }
                let digits = &body[..width];
                if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                let val = u32::from_str_radix(digits, 16).ok()?;
                if val > 0x10_ffff {
                    return None;
                }
                body = &body[width..];
                if braced {
                    body = &body[1..];
                }
                val
            }
            other => other as u32,
        };
        if (0xd800..=0xdfff).contains(&decoded_code) {
            units.push(decoded_code as u16);
        } else if let Some(ch) = char::from_u32(decoded_code) {
            let mut buf = [0u16; 2];
            units.extend_from_slice(ch.encode_utf16(&mut buf));
        } else {
            return None;
        }
    }

    let mut out = String::new();
    let mut raw_inner = String::new();
    let mut has_lone_surrogate = false;
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xd800..=0xdbff).contains(&u)
            && i + 1 < units.len()
            && (0xdc00..=0xdfff).contains(&units[i + 1])
        {
            let high = u32::from(u - 0xd800);
            let low = u32::from(units[i + 1] - 0xdc00);
            if let Some(ch) = char::from_u32(0x10000 + (high << 10) + low) {
                out.push(ch);
                push_json_escaped_char(&mut raw_inner, ch);
            }
            i += 2;
            continue;
        }
        if (0xd800..=0xdfff).contains(&u) {
            has_lone_surrogate = true;
            let esc = format!(r"\u{u:04x}");
            out.push_str(&esc);
            raw_inner.push_str(&esc);
        } else if let Some(ch) = char::from_u32(u32::from(u)) {
            out.push(ch);
            push_json_escaped_char(&mut raw_inner, ch);
        }
        i += 1;
    }
    let raw_json = has_lone_surrogate.then(|| format!("\"{raw_inner}\""));
    Some(DecodedJsString {
        display: out,
        raw_json,
    })
}

fn escape_json_pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn json_string_inner_escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        push_json_escaped_char(&mut out, ch);
    }
    out
}

/// Strict RFC 8259 JSON parser tracking byte offsets and allowing duplicate keys.
struct StrictJsonParser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    collect_tokens: bool,
    entries: Vec<(String, Option<String>, &'static str, usize, usize)>,
    tokens: Vec<Token>,
}

impl<'a> StrictJsonParser<'a> {
    fn new(src: &'a str, collect_tokens: bool) -> Self {
        let pos = if src.starts_with('\u{feff}') { 3 } else { 0 };
        Self {
            src,
            bytes: src.as_bytes(),
            pos,
            collect_tokens,
            entries: Vec::new(),
            tokens: Vec::new(),
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn parse_root(
        mut self,
    ) -> Result<
        (
            Vec<(String, Option<String>, &'static str, usize, usize)>,
            Vec<Token>,
        ),
        usize,
    > {
        self.skip_ws();
        self.parse_value("", None)?;
        self.skip_ws();
        if self.pos < self.bytes.len() {
            return Err(self.pos);
        }
        Ok((self.entries, self.tokens))
    }

    fn parse_value(&mut self, pointer: &str, raw_pointer_inner: Option<&str>) -> Result<(), usize> {
        self.skip_ws();
        let from = self.pos;
        if from >= self.bytes.len() {
            return Err(from);
        }
        let raw_ptr_json = raw_pointer_inner.map(|inner| format!("\"{inner}\""));
        match self.bytes[from] {
            b'{' => {
                let idx = self.entries.len();
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "object", from, from));
                self.pos += 1;
                self.skip_ws();
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
                    self.pos += 1;
                    self.entries[idx].4 = self.pos;
                    return Ok(());
                }
                loop {
                    self.skip_ws();
                    let key = self.parse_string_parts()?;
                    self.skip_ws();
                    if self.pos >= self.bytes.len() || self.bytes[self.pos] != b':' {
                        return Err(self.pos);
                    }
                    self.pos += 1;
                    let child_ptr = format!("{pointer}/{}", escape_json_pointer(&key.display));
                    let child_raw_inner = if raw_pointer_inner.is_some() || key.raw_json.is_some() {
                        let parent_inner = raw_pointer_inner
                            .map(str::to_string)
                            .unwrap_or_else(|| json_string_inner_escaped(pointer));
                        let key_inner = key
                            .raw_json
                            .as_deref()
                            .map(|raw| &raw[1..raw.len() - 1])
                            .map(escape_json_pointer)
                            .unwrap_or_else(|| {
                                json_string_inner_escaped(&escape_json_pointer(&key.display))
                            });
                        Some(format!("{parent_inner}/{key_inner}"))
                    } else {
                        None
                    };
                    self.parse_value(&child_ptr, child_raw_inner.as_deref())?;
                    self.skip_ws();
                    if self.pos >= self.bytes.len() {
                        return Err(self.pos);
                    }
                    match self.bytes[self.pos] {
                        b',' => self.pos += 1,
                        b'}' => {
                            self.pos += 1;
                            self.entries[idx].4 = self.pos;
                            return Ok(());
                        }
                        _ => return Err(self.pos),
                    }
                }
            }
            b'[' => {
                let idx = self.entries.len();
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "array", from, from));
                self.pos += 1;
                self.skip_ws();
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b']' {
                    self.pos += 1;
                    self.entries[idx].4 = self.pos;
                    return Ok(());
                }
                let mut elem_index = 0usize;
                loop {
                    let child_ptr = format!("{pointer}/{elem_index}");
                    let child_raw_inner = raw_pointer_inner
                        .map(|parent_inner| format!("{parent_inner}/{elem_index}"));
                    self.parse_value(&child_ptr, child_raw_inner.as_deref())?;
                    elem_index += 1;
                    self.skip_ws();
                    if self.pos >= self.bytes.len() {
                        return Err(self.pos);
                    }
                    match self.bytes[self.pos] {
                        b',' => self.pos += 1,
                        b']' => {
                            self.pos += 1;
                            self.entries[idx].4 = self.pos;
                            return Ok(());
                        }
                        _ => return Err(self.pos),
                    }
                }
            }
            b'"' => {
                let _ = self.parse_string_parts()?;
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "string", from, self.pos));
                Ok(())
            }
            b't' => {
                self.expect_literal(b"true")?;
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "boolean", from, self.pos));
                Ok(())
            }
            b'f' => {
                self.expect_literal(b"false")?;
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "boolean", from, self.pos));
                Ok(())
            }
            b'n' => {
                self.expect_literal(b"null")?;
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "null", from, self.pos));
                Ok(())
            }
            b'-' | b'0'..=b'9' => {
                self.parse_number()?;
                self.entries
                    .push((pointer.to_string(), raw_ptr_json, "number", from, self.pos));
                Ok(())
            }
            _ => Err(from),
        }
    }

    fn expect_literal(&mut self, lit: &[u8]) -> Result<(), usize> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(())
        } else {
            Err(self.pos)
        }
    }

    fn parse_string_parts(&mut self) -> Result<DecodedJsString, usize> {
        let from = self.pos;
        if self.pos >= self.bytes.len() || self.bytes[self.pos] != b'"' {
            return Err(from);
        }
        self.pos += 1;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    let raw = &self.src[from..self.pos];
                    let Some(decoded) = decode_js_string_parts(raw) else {
                        return Err(from);
                    };
                    if self.collect_tokens {
                        self.tokens.push(Token {
                            from: from + 1,
                            text: decoded.display.clone(),
                        });
                    }
                    return Ok(decoded);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.bytes.len() {
                        return Err(self.pos);
                    }
                    match self.bytes[self.pos] {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            self.pos += 1;
                        }
                        b'u' => {
                            self.pos += 1;
                            if self.pos + 4 > self.bytes.len()
                                || !self.bytes[self.pos..self.pos + 4]
                                    .iter()
                                    .all(u8::is_ascii_hexdigit)
                            {
                                return Err(self.pos);
                            }
                            self.pos += 4;
                        }
                        _ => return Err(self.pos),
                    }
                }
                0x00..=0x1f => return Err(self.pos),
                _ => self.pos += 1,
            }
        }
        Err(self.pos)
    }

    fn parse_number(&mut self) -> Result<(), usize> {
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        match self.bytes.get(self.pos) {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.pos),
        }
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if !self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                return Err(self.pos);
            }
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
        }
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                return Err(self.pos);
            }
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
        }
        Ok(())
    }
}

fn decode_strict_json_string_parts(raw: &str) -> Option<DecodedJsString> {
    let mut parser = StrictJsonParser::new(raw, /*collect_tokens*/ false);
    let val = parser.parse_string_parts().ok()?;
    if parser.pos == raw.len() {
        Some(val)
    } else {
        None
    }
}

fn decode_strict_json_string(raw: &str) -> Option<String> {
    decode_strict_json_string_parts(raw).map(|v| v.display)
}

pub(crate) fn parse_json(doc: &mut Source, collect_tokens: bool) -> Result<(), String> {
    match StrictJsonParser::new(&doc.source, collect_tokens).parse_root() {
        Ok((entries, tokens)) => {
            for (pointer, pointer_raw_json, value_type, from, to) in entries {
                doc.add("json", String::new(), from, to, None, None, true);
                if let Some(last) = doc.entries.last_mut() {
                    last.pointer = pointer;
                    last.pointer_raw_json = pointer_raw_json;
                    last.value_type = value_type.to_string();
                }
            }
            doc.tokens = tokens;
            Ok(())
        }
        Err(invalid_offset) => {
            let mut data = doc.source.as_bytes().to_vec();
            if doc.source.starts_with('\u{feff}') && data.len() >= 3 {
                data[0] = b' ';
                data[1] = b' ';
                data[2] = b' ';
            }
            let tree = parse_tree(&data, tree_sitter_json::LANGUAGE.into())?;
            for node in named_children(tree.root_node()) {
                if matches!(
                    node.kind(),
                    "object" | "array" | "string" | "number" | "true" | "false" | "null"
                ) {
                    collect_json_recovery(doc, node, "", None);
                }
                if !doc.entries.is_empty() {
                    break;
                }
            }
            if collect_tokens {
                collect_json_recovery_tokens(doc, tree.root_node());
            }
            doc.parse_error(invalid_offset);
            Ok(())
        }
    }
}

fn collect_json_recovery(
    doc: &mut Source,
    node: Node<'_>,
    pointer: &str,
    raw_pointer_inner: Option<&str>,
) {
    let kind = node.kind();
    let value_type = match kind {
        "object" | "array" | "string" | "number" | "null" => kind,
        "true" | "false" => "boolean",
        _ => return,
    };
    doc.add(
        "json",
        String::new(),
        node.start_byte(),
        node.end_byte(),
        None,
        None,
        !node.has_error(),
    );
    if let Some(last) = doc.entries.last_mut() {
        last.pointer = pointer.to_string();
        last.pointer_raw_json = raw_pointer_inner.map(|inner| format!("\"{inner}\""));
        last.value_type = value_type.to_string();
    }

    if kind == "object" {
        for pair in named_children(node) {
            if pair.kind() != "pair" {
                continue;
            }
            let (Some(key), Some(val)) = (
                pair.child_by_field_name("key"),
                pair.child_by_field_name("value"),
            ) else {
                continue;
            };
            if key.has_error() {
                continue;
            }
            let Some(decoded_key) = decode_strict_json_string_parts(doc.text(key)) else {
                continue;
            };
            let valid = named_children(pair)
                .into_iter()
                .all(|part| !(part.start_byte() < val.start_byte() && part.is_error()));
            if valid {
                let child_ptr = format!("{pointer}/{}", escape_json_pointer(&decoded_key.display));
                let child_raw_inner = if raw_pointer_inner.is_some()
                    || decoded_key.raw_json.is_some()
                {
                    let parent_inner = raw_pointer_inner
                        .map(str::to_string)
                        .unwrap_or_else(|| json_string_inner_escaped(pointer));
                    let key_inner = decoded_key
                        .raw_json
                        .as_deref()
                        .map(|raw| &raw[1..raw.len() - 1])
                        .map(escape_json_pointer)
                        .unwrap_or_else(|| {
                            json_string_inner_escaped(&escape_json_pointer(&decoded_key.display))
                        });
                    Some(format!("{parent_inner}/{key_inner}"))
                } else {
                    None
                };
                collect_json_recovery(doc, val, &child_ptr, child_raw_inner.as_deref());
            }
        }
    } else if kind == "array" {
        let mut index = 0usize;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "," {
                index += 1;
            } else {
                let child_ptr = format!("{pointer}/{index}");
                let child_raw_inner =
                    raw_pointer_inner.map(|parent_inner| format!("{parent_inner}/{index}"));
                collect_json_recovery(doc, child, &child_ptr, child_raw_inner.as_deref());
            }
        }
    }
}

fn collect_json_recovery_tokens(doc: &mut Source, node: Node<'_>) {
    if node.kind() == "string" && !node.has_error() {
        if let Some(decoded) = decode_strict_json_string(doc.text(node)) {
            doc.tokens.push(Token {
                from: node.start_byte() + 1,
                text: decoded,
            });
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_json_recovery_tokens(doc, child);
    }
}

fn yaml_error_line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        #[allow(clippy::expect_used)]
        Regex::new(r"line ([0-9]+)").expect("valid yaml error line regex")
    })
}

/// Extract top-level YAML frontmatter keys and ATX Markdown headings.
pub(crate) fn parse_markdown(doc: &mut Source) {
    let mut end_front = 0usize;
    let total_lines = doc.lines.len();
    if let Some(first) = doc.lines.row(1)
        && first.strip_prefix('\u{feff}').unwrap_or(first) == "---"
    {
        let mut closing = 0usize;
        for i in 2..=total_lines {
            if doc.lines.row(i) == Some("---") {
                closing = i;
                break;
            }
        }
        if closing > 0
            && let (Some(start), Some(end)) = (doc.lines.start_of(2), doc.lines.start_of(closing))
        {
            end_front = if closing < total_lines {
                doc.lines.start_of(closing + 1).unwrap_or(doc.source.len())
            } else {
                doc.source.len()
            };
            let yaml_slice = &doc.source[start..end];
            let yaml_valid = match serde_yaml::from_str::<serde_yaml::Value>(yaml_slice) {
                Ok(val) => Some(val),
                Err(err) => {
                    let msg = err.to_string();
                    let line = yaml_error_line_regex()
                        .captures(&msg)
                        .and_then(|c| c.get(1)?.as_str().parse::<usize>().ok())
                        .map(|n| (n + 1).min(total_lines))
                        .unwrap_or(2);
                    if let Some(offset) = doc.lines.start_of(line) {
                        doc.parse_error(offset);
                    }
                    None
                }
            };
            if yaml_valid.is_some_and(|v| v.is_mapping()) {
                extract_yaml_top_level_keys(doc, 2, closing);
            }
        }
    }

    // Normalize bare CR to LF without shifting byte offsets for pulldown-cmark.
    let mut normalized = doc.source.clone();
    // Safety: replacing single ASCII byte '\r' with ASCII '\n' preserves UTF-8 validity.
    unsafe {
        let bytes = normalized.as_bytes_mut();
        let len = bytes.len();
        for i in 0..len {
            if bytes[i] == b'\r' && (i + 1 == len || bytes[i + 1] != b'\n') {
                bytes[i] = b'\n';
            }
        }
    }

    let parser = MdParser::new(&normalized).into_offset_iter();
    for (event, range) in parser {
        let Event::Start(Tag::Heading { level, .. }) = event else {
            continue;
        };
        if range.start < end_front {
            continue;
        }
        let level_num = heading_level_num(level);
        let line = doc.lines.at(range.start);
        let Some(line_start) = doc.lines.start_of(line) else {
            continue;
        };
        let Some(line_end) = doc.lines.end_of(line) else {
            continue;
        };
        let slice = &doc.source[range.start..line_end.min(range.end)];
        let hashes = slice.bytes().take_while(|&b| b == b'#').count();
        if hashes != level_num {
            continue;
        }
        let after_hashes = &slice[hashes..];
        if !after_hashes.is_empty() && !after_hashes.starts_with([' ', '\t']) {
            continue;
        }
        let name = trim_atx_closing_hashes(after_hashes.trim());
        if !name.is_empty() {
            let from = range.start.max(line_start);
            doc.add(
                "heading",
                name.to_string(),
                from,
                line_end,
                None,
                None,
                true,
            );
            if let Some(last) = doc.entries.last_mut() {
                last.level = level_num;
            }
        }
    }
}

fn heading_level_num(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn trim_atx_closing_hashes(s: &str) -> &str {
    let trimmed = s.trim_end_matches([' ', '\t']);
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() < trimmed.len()
        && (without_hashes.is_empty() || without_hashes.ends_with([' ', '\t']))
    {
        without_hashes.trim()
    } else {
        trimmed
    }
}

/// Extract top-level mapping keys from validated YAML frontmatter lines `start_line..closing_line`.
fn extract_yaml_top_level_keys(doc: &mut Source, start_line: usize, closing_line: usize) {
    if let (Some(start), Some(end)) = (
        doc.lines.start_of(start_line),
        doc.lines.start_of(closing_line),
    ) && extract_yaml_flow_mapping_keys(doc, start, end)
    {
        return;
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut in_block_scalar = false;
    for line_num in start_line..closing_line {
        let Some(row) = doc.lines.row(line_num) else {
            continue;
        };
        if row.is_empty() || row.starts_with([' ', '\t']) {
            continue;
        }
        if in_block_scalar {
            in_block_scalar = false;
        }
        if row.starts_with('#') || row == "..." {
            continue;
        }
        let Some(line_start) = doc.lines.start_of(line_num) else {
            continue;
        };
        let Some((key, key_len, rest)) = parse_yaml_top_key(row) else {
            continue;
        };
        let rest_trimmed = rest.trim();
        let starts_block = rest_trimmed.starts_with('|') || rest_trimmed.starts_with('>');
        if !key.is_empty() && key != "null" && key != "~" {
            let to = (line_start + key_len).min(doc.source.len());
            doc.add("frontmatter", key.clone(), line_start, to, None, None, true);
            if !seen.insert(key) {
                doc.parse_error(line_start);
            }
        }
        if starts_block {
            in_block_scalar = true;
        }
    }
}

/// Parse top-level keys of a flow-style YAML mapping (`{k1: v1, k2: v2}`) inside `start..end`.
/// Returns `true` if the frontmatter begins with a flow mapping.
fn extract_yaml_flow_mapping_keys(doc: &mut Source, start: usize, end: usize) -> bool {
    let slice = &doc.source[start..end];
    let bytes = slice.as_bytes();
    let mut pos = 0usize;
    skip_yaml_flow_ws_and_comments(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return false;
    }
    pos += 1;
    let mut seen: HashSet<String> = HashSet::new();
    let mut keys: Vec<(String, usize, usize)> = Vec::new();
    loop {
        skip_yaml_flow_ws_and_comments(bytes, &mut pos);
        if pos >= bytes.len() || bytes[pos] == b'}' {
            break;
        }
        let key_start = pos;
        let (key, key_end) = match bytes[pos] {
            b'"' | b'\'' => {
                let quote = bytes[pos];
                pos += 1;
                while pos < bytes.len() {
                    if quote == b'"' && bytes[pos] == b'\\' {
                        pos = (pos + 2).min(bytes.len());
                        continue;
                    }
                    if bytes[pos] == quote {
                        pos += 1;
                        if quote == b'\'' && pos < bytes.len() && bytes[pos] == b'\'' {
                            pos += 1;
                            continue;
                        }
                        break;
                    }
                    pos += 1;
                }
                let raw_key = &slice[key_start..pos];
                let decoded = if quote == b'"' {
                    serde_yaml::from_str::<String>(raw_key)
                        .unwrap_or_else(|_| raw_key[1..raw_key.len().saturating_sub(1)].to_string())
                } else {
                    raw_key[1..raw_key.len().saturating_sub(1)].replace("''", "'")
                };
                (decoded, pos)
            }
            _ => {
                while pos < bytes.len() && !matches!(bytes[pos], b':' | b',' | b'}' | b'\r' | b'\n')
                {
                    pos += 1;
                }
                let raw_key = slice[key_start..pos].trim_end_matches([' ', '\t']);
                (raw_key.to_string(), key_start + raw_key.len())
            }
        };
        skip_yaml_flow_ws_and_comments(bytes, &mut pos);
        if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
        }
        if !key.is_empty() && key != "null" && key != "~" {
            keys.push((key, start + key_start, start + key_end));
        }
        // Skip the value at flow depth 0 until `,` or `}`.
        // Quotes only start a quoted scalar at token start, never mid-word (e.g. `it's nice`).
        let mut depth = 0usize;
        let mut at_token_start = true;
        while pos < bytes.len() {
            match bytes[pos] {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    pos += 1;
                }
                b'"' | b'\'' if at_token_start => {
                    let quote = bytes[pos];
                    pos += 1;
                    while pos < bytes.len() {
                        if quote == b'"' && bytes[pos] == b'\\' {
                            pos = (pos + 2).min(bytes.len());
                            continue;
                        }
                        if bytes[pos] == quote {
                            pos += 1;
                            if quote == b'\'' && pos < bytes.len() && bytes[pos] == b'\'' {
                                pos += 1;
                                continue;
                            }
                            break;
                        }
                        pos += 1;
                    }
                    at_token_start = false;
                }
                b'{' | b'[' => {
                    depth += 1;
                    pos += 1;
                    at_token_start = true;
                }
                b'}' | b']' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    pos += 1;
                    at_token_start = false;
                }
                b',' if depth == 0 => {
                    pos += 1;
                    break;
                }
                b',' | b':' => {
                    pos += 1;
                    at_token_start = true;
                }
                b'#' if pos == 0 || matches!(bytes[pos - 1], b' ' | b'\t' | b'\r' | b'\n') => {
                    while pos < bytes.len() && bytes[pos] != b'\n' {
                        pos += 1;
                    }
                }
                _ => {
                    pos += 1;
                    at_token_start = false;
                }
            }
        }
    }
    for (key, from, to) in keys {
        doc.add("frontmatter", key.clone(), from, to, None, None, true);
        if !seen.insert(key) {
            doc.parse_error(from);
        }
    }
    true
}

fn skip_yaml_flow_ws_and_comments(bytes: &[u8], pos: &mut usize) {
    loop {
        while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\r' | b'\n') {
            *pos += 1;
        }
        if *pos < bytes.len() && bytes[*pos] == b'#' {
            while *pos < bytes.len() && bytes[*pos] != b'\n' {
                *pos += 1;
            }
        } else {
            break;
        }
    }
}

fn parse_yaml_top_key(row: &str) -> Option<(String, usize, &str)> {
    let bytes = row.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let quote = bytes[0];
        let mut i = 1;
        while i < bytes.len() {
            if quote == b'"' && bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                i += 1;
                if quote == b'\'' && i < bytes.len() && bytes[i] == b'\'' {
                    i += 1;
                    continue;
                }
                break;
            }
            i += 1;
        }
        let raw_key = &row[..i];
        let after = row[i..].trim_start_matches([' ', '\t']);
        let rest = after.strip_prefix(':')?;
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            return None;
        }
        let decoded = if quote == b'"' {
            serde_yaml::from_str::<String>(raw_key).ok()?
        } else {
            raw_key[1..raw_key.len() - 1].replace("''", "'")
        };
        return Some((decoded, i, rest));
    }

    // Unquoted scalar key: find `:` followed by whitespace or EOL.
    let mut colon_pos = None;
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b':' && (idx + 1 == bytes.len() || matches!(bytes[idx + 1], b' ' | b'\t')) {
            colon_pos = Some(idx);
            break;
        }
    }
    let idx = colon_pos?;
    let key = row[..idx].trim_end_matches([' ', '\t']);
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), key.len(), &row[idx + 1..]))
}
