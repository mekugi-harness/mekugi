use tree_sitter::Node;

use super::Source;
use super::Token;
use super::named_children;
use super::normalize_parser_bytes;
use super::parse_tree;

/// Parse Go declarations and lexical identifiers using `tree-sitter-go`,
/// matching `libraries/sourceanalysis/gooutline`.
pub(crate) fn parse_go(doc: &mut Source, collect_tokens: bool) -> Result<(), String> {
    let data = normalize_parser_bytes(&doc.source);
    let tree = parse_tree(&data, tree_sitter_go::LANGUAGE.into())?;
    let root = tree.root_node();

    for top in named_children(root) {
        go_declaration(doc, top);
    }
    visit_go_errors_and_tokens(doc, root, &data, collect_tokens);
    Ok(())
}

fn go_declaration(doc: &mut Source, node: Node<'_>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name) = node.child_by_field_name("name")
                && !name.is_missing()
            {
                let text = doc.text(name).to_string();
                doc.add(
                    "function",
                    text,
                    node.start_byte(),
                    node.end_byte(),
                    Some(name.start_byte()),
                    Some(name.end_byte()),
                    !node.has_error(),
                );
            }
        }
        "method_declaration" => {
            if let Some(name) = node.child_by_field_name("name")
                && !name.is_missing()
            {
                let receiver = node
                    .child_by_field_name("receiver")
                    .and_then(|recv| extract_go_receiver_type(doc, recv))
                    .unwrap_or_default();
                let text = doc.text(name).to_string();
                doc.add(
                    "method",
                    text,
                    node.start_byte(),
                    node.end_byte(),
                    Some(name.start_byte()),
                    Some(name.end_byte()),
                    !node.has_error(),
                );
                if let Some(last) = doc.entries.last_mut() {
                    last.receiver = receiver;
                }
            }
        }
        "import_declaration" => {
            collect_go_imports(doc, node);
        }
        "type_declaration" => {
            for child in named_children(node) {
                if matches!(child.kind(), "type_spec" | "type_alias")
                    && let Some(name) = child.child_by_field_name("name")
                    && !name.is_missing()
                {
                    let text = doc.text(name).to_string();
                    doc.add(
                        "type",
                        text,
                        child.start_byte(),
                        child.end_byte(),
                        Some(name.start_byte()),
                        Some(name.end_byte()),
                        !child.has_error(),
                    );
                }
            }
        }
        "const_declaration" => {
            for child in named_children(node) {
                if child.kind() == "const_spec" {
                    add_go_value_spec(doc, child, "constant");
                }
            }
        }
        "var_declaration" => {
            for child in named_children(node) {
                if child.kind() == "var_spec" {
                    add_go_value_spec(doc, child, "variable");
                }
            }
        }
        "ERROR" => {
            for child in named_children(node) {
                go_declaration(doc, child);
            }
        }
        _ => {}
    }
}

fn add_go_value_spec(doc: &mut Source, spec: Node<'_>, kind: &str) {
    let mut cursor = spec.walk();
    for name in spec.children_by_field_name("name", &mut cursor) {
        if !name.is_missing() {
            let text = doc.text(name).to_string();
            doc.add(
                kind,
                text,
                spec.start_byte(),
                spec.end_byte(),
                Some(name.start_byte()),
                Some(name.end_byte()),
                !spec.has_error(),
            );
        }
    }
}

fn collect_go_imports(doc: &mut Source, node: Node<'_>) {
    for child in named_children(node) {
        match child.kind() {
            "import_spec" => {
                if let Some(path) = child.child_by_field_name("path")
                    && let Some(unquoted) = unquote_go_string(doc.text(path))
                {
                    doc.add(
                        "import",
                        unquoted,
                        child.start_byte(),
                        child.end_byte(),
                        None,
                        None,
                        !child.has_error(),
                    );
                }
            }
            "import_spec_list" => collect_go_imports(doc, child),
            _ => {}
        }
    }
}

/// Decode a Go raw or interpreted string literal matching `strconv.Unquote`.
fn unquote_go_string(raw: &str) -> Option<String> {
    if raw.len() < 2 {
        return None;
    }
    if raw.starts_with('`') && raw.ends_with('`') {
        let inner = &raw[1..raw.len() - 1];
        if inner.contains('`') {
            return None;
        }
        return Some(inner.replace('\r', ""));
    }
    if !(raw.starts_with('"') && raw.ends_with('"')) {
        return None;
    }
    let mut bytes = Vec::with_capacity(raw.len() - 2);
    let mut body = &raw[1..raw.len() - 1];
    while let Some(ch) = body.chars().next() {
        body = &body[ch.len_utf8()..];
        if ch == '"' || ch == '\n' {
            return None;
        }
        if ch != '\\' {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let esc = body.chars().next()?;
        body = &body[esc.len_utf8()..];
        match esc {
            'a' => bytes.push(0x07),
            'b' => bytes.push(0x08),
            'f' => bytes.push(0x0c),
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            't' => bytes.push(b'\t'),
            'v' => bytes.push(0x0b),
            '\\' => bytes.push(b'\\'),
            '"' => bytes.push(b'"'),
            '0'..='7' => {
                if body.len() < 2 {
                    return None;
                }
                let d1 = body.as_bytes()[0];
                let d2 = body.as_bytes()[1];
                if !(b'0'..=b'7').contains(&d1) || !(b'0'..=b'7').contains(&d2) {
                    return None;
                }
                let val = u32::from(esc as u8 - b'0') * 64
                    + u32::from(d1 - b'0') * 8
                    + u32::from(d2 - b'0');
                if val > 0xff {
                    return None;
                }
                bytes.push(val as u8);
                body = &body[2..];
            }
            'x' => {
                if body.len() < 2 || !body.as_bytes()[..2].iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                let hex = &body[..2];
                let val = u8::from_str_radix(hex, 16).ok()?;
                bytes.push(val);
                body = &body[2..];
            }
            'u' | 'U' => {
                let width = if esc == 'u' { 4 } else { 8 };
                if body.len() < width || !body.as_bytes()[..width].iter().all(u8::is_ascii_hexdigit)
                {
                    return None;
                }
                let hex = &body[..width];
                let val = u32::from_str_radix(hex, 16).ok()?;
                let decoded = char::from_u32(val)?;
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(decoded.encode_utf8(&mut buf).as_bytes());
                body = &body[width..];
            }
            _ => return None,
        }
    }
    String::from_utf8(bytes).ok()
}

fn extract_go_receiver_type(doc: &Source, param_list: Node<'_>) -> Option<String> {
    let param = named_children(param_list)
        .into_iter()
        .find(|c| c.kind() == "parameter_declaration")?;
    let ty = param.child_by_field_name("type")?;
    // Compact internal whitespace in receiver types to match `go/format`.
    let raw = doc.text(ty).trim();
    Some(
        raw.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace("* ", "*"),
    )
}

fn visit_go_errors_and_tokens(doc: &mut Source, node: Node<'_>, data: &[u8], collect_tokens: bool) {
    if !collect_tokens && !node.has_error() && !node.is_error() && !node.is_missing() {
        return;
    }
    if node.is_error() || node.is_missing() {
        doc.parse_error(node.start_byte());
    }
    if collect_tokens
        && matches!(
            node.kind(),
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "package_identifier"
                | "label_name"
                | "blank_identifier"
        )
        && let Ok(text) = node.utf8_text(data)
    {
        doc.tokens.push(Token {
            from: node.start_byte(),
            text: text.to_string(),
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_go_errors_and_tokens(doc, child, data, collect_tokens);
    }
}

/// Check whether `name` is a valid non-keyword Go identifier (matching `go/token.IsIdentifier`).
pub fn is_go_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '_' && !first.is_alphabetic() {
        return false;
    }
    if !chars.all(|ch| ch == '_' || ch.is_alphabetic() || ch.is_numeric()) {
        return false;
    }
    !matches!(
        name,
        "break"
            | "case"
            | "chan"
            | "const"
            | "continue"
            | "default"
            | "defer"
            | "else"
            | "fallthrough"
            | "for"
            | "func"
            | "go"
            | "goto"
            | "if"
            | "import"
            | "interface"
            | "map"
            | "package"
            | "range"
            | "return"
            | "select"
            | "struct"
            | "switch"
            | "type"
            | "var"
    )
}
