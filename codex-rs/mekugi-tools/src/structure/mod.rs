mod document;
mod go;

pub use go::is_go_identifier;

use serde::Deserialize;
use serde::Serialize;
use serde::ser::SerializeMap;
use tree_sitter::Language;
use tree_sitter::Node;
use tree_sitter::Parser;

use crate::sourcekind::SourceFormat;

/// Zero-based logical line and UTF-16 code-unit coordinate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

#[derive(Clone, Debug)]
pub struct Lines {
    text: String,
    starts: Vec<usize>,
    ends: Vec<usize>,
}

impl Lines {
    pub fn new(text: String) -> Self {
        let bytes = text.as_bytes();
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        let mut start = 0usize;
        while start < bytes.len() {
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'\r' && bytes[end] != b'\n' {
                end += 1;
            }
            starts.push(start);
            ends.push(end);
            let mut next = end;
            if next < bytes.len() {
                next += 1;
                if bytes[end] == b'\r' && next < bytes.len() && bytes[next] == b'\n' {
                    next += 1;
                }
            }
            start = next;
        }
        Self { text, starts, ends }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn start_of(&self, line_1_based: usize) -> Option<usize> {
        self.starts.get(line_1_based.checked_sub(1)?).copied()
    }

    pub fn end_of(&self, line_1_based: usize) -> Option<usize> {
        self.ends.get(line_1_based.checked_sub(1)?).copied()
    }

    /// 1-based line number for a byte offset.
    pub fn at(&self, offset: usize) -> usize {
        let idx = self.starts.partition_point(|&s| s <= offset);
        idx.max(1)
    }

    /// 1-based line text without line terminator.
    pub fn row(&self, line_1_based: usize) -> Option<&str> {
        let idx = line_1_based.checked_sub(1)?;
        let start = *self.starts.get(idx)?;
        let end = *self.ends.get(idx)?;
        Some(&self.text[start..end])
    }

    /// Convert byte offset into zero-based LSP UTF-16 Position.
    pub fn position(&self, offset: usize) -> Position {
        let line = self.at(offset).saturating_sub(1);
        let line_start = self.starts.get(line).copied().unwrap_or(0);
        let clamped = offset.clamp(line_start, self.text.len());
        let character: usize = self.text[line_start..clamped]
            .chars()
            .map(char::len_utf16)
            .sum();
        Position { line, character }
    }

    /// Convert zero-based LSP UTF-16 Position into byte offset.
    pub fn offset(&self, pos: Position) -> Option<usize> {
        let start = *self.starts.get(pos.line)?;
        let end = *self.ends.get(pos.line)?;
        let mut offset = start;
        let mut remaining = pos.character;
        for ch in self.text[start..end].chars() {
            if remaining == 0 {
                break;
            }
            let units = ch.len_utf16();
            if units > remaining {
                return None;
            }
            remaining -= units;
            offset += ch.len_utf8();
        }
        if remaining == 0 { Some(offset) } else { None }
    }
}

/// One structural navigation entry with 1-based inclusive line spans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub name_raw_json: Option<String>,
    pub receiver: String,
    pub pointer: String,
    pub pointer_raw_json: Option<String>,
    pub value_type: String,
    pub line: usize,
    pub end_line: usize,
    pub level: usize,
    pub from: usize,
    pub name_from: Option<usize>,
    pub name_to: Option<usize>,
    pub complete: bool,
}

impl Serialize for Entry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.kind.as_str() {
            "method" => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("kind", &self.kind)?;
                map.serialize_entry("name", &self.name)?;
                map.serialize_entry("receiver", &self.receiver)?;
                map.serialize_entry("line", &self.line)?;
                map.serialize_entry("line_end", &self.end_line)?;
                map.end()
            }
            "json" => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("kind", &self.kind)?;
                map.serialize_entry("pointer", &self.pointer)?;
                map.serialize_entry("value_type", &self.value_type)?;
                map.serialize_entry("line", &self.line)?;
                map.serialize_entry("line_end", &self.end_line)?;
                map.end()
            }
            "heading" => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("kind", &self.kind)?;
                map.serialize_entry("name", &self.name)?;
                map.serialize_entry("level", &self.level)?;
                map.serialize_entry("line", &self.line)?;
                map.serialize_entry("line_end", &self.end_line)?;
                map.end()
            }
            _ => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("kind", &self.kind)?;
                map.serialize_entry("name", &self.name)?;
                map.serialize_entry("line", &self.line)?;
                map.serialize_entry("line_end", &self.end_line)?;
                map.end()
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub from: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub source: String,
    pub lines: Lines,
    pub format: SourceFormat,
    pub entries: Vec<Entry>,
    pub tokens: Vec<Token>,
}

impl Source {
    fn new(source: String, format: SourceFormat) -> Self {
        let lines = Lines::new(source.clone());
        Self {
            source,
            lines,
            format,
            entries: Vec::new(),
            tokens: Vec::new(),
        }
    }

    pub(crate) fn add(
        &mut self,
        kind: &str,
        name: String,
        from: usize,
        to: usize,
        name_from: Option<usize>,
        name_to: Option<usize>,
        complete: bool,
    ) {
        let end_offset = to.saturating_sub(1).max(from);
        let line = self.lines.at(from);
        let end_line = self.lines.at(end_offset);
        self.entries.push(Entry {
            kind: kind.to_string(),
            name,
            name_raw_json: None,
            receiver: String::new(),
            pointer: String::new(),
            pointer_raw_json: None,
            value_type: String::new(),
            line,
            end_line,
            level: 0,
            from,
            name_from,
            name_to,
            complete,
        });
    }

    pub(crate) fn parse_error(&mut self, offset: usize) {
        let clamped = offset.min(self.source.len().saturating_sub(1));
        let line = self.lines.at(clamped);
        if self
            .entries
            .iter()
            .any(|e| e.kind == "parse_error" && e.line == line)
        {
            return;
        }
        self.add(
            "parse_error",
            "syntax error".to_string(),
            clamped,
            clamped,
            None,
            None,
            false,
        );
    }

    pub(crate) fn text(&self, node: Node<'_>) -> &str {
        &self.source[node.start_byte()..node.end_byte()]
    }

    fn add_node(
        &mut self,
        kind: &str,
        node: Node<'_>,
        span: Node<'_>,
        name: Option<Node<'_>>,
        receiver: &str,
    ) {
        let Some(name) = name else {
            return;
        };
        if name.is_missing() {
            return;
        }
        let text = self.text(name).to_string();
        self.add(
            kind,
            text,
            span.start_byte(),
            span.end_byte(),
            Some(name.start_byte()),
            Some(name.end_byte()),
            !node.has_error() && !span.has_error(),
        );
        if let Some(last) = self.entries.last_mut() {
            last.receiver = receiver.to_string();
        }
    }

    fn code_outline(&mut self, root: Node<'_>) {
        if self.format.language == Some("rust") {
            self.rust_declarations(root, "");
            return;
        }
        for top in named_children(root) {
            self.code_declaration(top, top);
        }
    }

    fn rust_declarations(&mut self, root: Node<'_>, receiver: &str) {
        let children: Vec<Node<'_>> = if root.is_error() {
            let mut out = Vec::new();
            let mut depth = 0usize;
            collect_rust_error_peers(root, &mut depth, &mut out);
            out
        } else {
            named_children(root)
        };

        let mut from: Option<usize> = None;
        for node in children {
            match node.kind() {
                "attribute_item" => {
                    if from.is_none() {
                        from = Some(node.start_byte());
                    }
                    continue;
                }
                "line_comment" | "block_comment" => continue,
                _ => {}
            }
            let start = from.unwrap_or_else(|| node.start_byte());
            self.rust_declaration(node, start, receiver);
            from = None;
        }
    }

    fn rust_declaration(&mut self, node: Node<'_>, from: usize, receiver: &str) {
        let add = |s: &mut Self, kind: &str, name: Option<Node<'_>>| {
            if let Some(name) = name
                && !name.is_missing()
            {
                let text = s.text(name).to_string();
                s.add(
                    kind,
                    text,
                    from,
                    node.end_byte(),
                    Some(name.start_byte()),
                    Some(name.end_byte()),
                    !node.has_error(),
                );
                if let Some(last) = s.entries.last_mut() {
                    last.receiver = receiver.to_string();
                }
            }
        };

        match node.kind() {
            "function_item" | "function_signature_item" => {
                let kind = if receiver.is_empty() {
                    "function"
                } else {
                    "method"
                };
                add(self, kind, node.child_by_field_name("name"));
            }
            "ERROR" => {
                self.rust_declarations(node, receiver);
            }
            "foreign_mod_item" => {
                if receiver.is_empty()
                    && let Some(body) = node.child_by_field_name("body")
                {
                    self.rust_declarations(body, "");
                }
            }
            "impl_item" => {
                if receiver.is_empty()
                    && let Some(body) = node.child_by_field_name("body")
                {
                    let recv = node
                        .child_by_field_name("type")
                        .map(|t| self.rust_receiver(t))
                        .unwrap_or_default();
                    self.rust_declarations(body, &recv);
                }
            }
            _ => {
                if !receiver.is_empty() {
                    return;
                }
                match node.kind() {
                    "struct_item" | "enum_item" | "union_item" | "type_item" | "trait_item" => {
                        let name = node.child_by_field_name("name");
                        add(self, "type", name);
                        if node.kind() == "trait_item"
                            && let Some(name_node) = name
                            && let Some(body) = node.child_by_field_name("body")
                        {
                            let trait_name = self.text(name_node).to_string();
                            self.rust_declarations(body, &trait_name);
                        }
                    }
                    "const_item" => add(self, "constant", node.child_by_field_name("name")),
                    "static_item" => add(self, "variable", node.child_by_field_name("name")),
                    "mod_item" => add(self, "module", node.child_by_field_name("name")),
                    "macro_definition" => add(self, "macro", node.child_by_field_name("name")),
                    "extern_crate_declaration" => {
                        let name = node
                            .child_by_field_name("alias")
                            .or_else(|| node.child_by_field_name("name"));
                        add(self, "import", name);
                    }
                    "use_declaration" => {
                        if let Some(arg) = node.child_by_field_name("argument") {
                            self.rust_use_argument(arg, None, from, node);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn rust_use_argument<'a>(
        &mut self,
        argument: Node<'a>,
        scope: Option<Node<'a>>,
        from: usize,
        decl: Node<'a>,
    ) {
        let path_name = |path: Option<Node<'a>>| -> Option<Node<'a>> {
            match path {
                Some(p) if p.kind() == "scoped_identifier" => p.child_by_field_name("name"),
                other => other,
            }
        };

        let mut add_import = |name: Option<Node<'_>>| {
            if let Some(name) = name
                && !name.is_missing()
            {
                let text = self.text(name).to_string();
                self.add(
                    "import",
                    text,
                    from,
                    decl.end_byte(),
                    Some(name.start_byte()),
                    Some(name.end_byte()),
                    !decl.has_error(),
                );
            }
        };

        match argument.kind() {
            "use_as_clause" => add_import(argument.child_by_field_name("alias")),
            "scoped_identifier" => {
                let mut name = argument.child_by_field_name("name");
                if let Some(n) = name
                    && n.kind() == "self"
                {
                    name = path_name(argument.child_by_field_name("path"));
                }
                add_import(name);
            }
            "self" => add_import(scope.or(Some(argument))),
            "identifier" | "super" | "crate" => add_import(Some(argument)),
            "use_wildcard" => {
                self.add(
                    "import",
                    "*".to_string(),
                    from,
                    decl.end_byte(),
                    None,
                    None,
                    !decl.has_error(),
                );
            }
            "scoped_use_list" => {
                if let Some(list) = argument.child_by_field_name("list") {
                    let next_scope = path_name(argument.child_by_field_name("path"));
                    self.rust_use_argument(list, next_scope, from, decl);
                }
            }
            "use_list" => {
                for child in named_children(argument) {
                    self.rust_use_argument(child, scope, from, decl);
                }
            }
            _ => {}
        }
    }

    fn rust_receiver(&self, node: Node<'_>) -> String {
        match node.kind() {
            "identifier" | "type_identifier" | "primitive_type" | "self" | "super" | "crate" => {
                self.text(node).to_string()
            }
            "generic_type" => node
                .child_by_field_name("type")
                .map(|t| self.rust_receiver(t))
                .unwrap_or_default(),
            "scoped_type_identifier" | "scoped_identifier" => {
                let path = node
                    .child_by_field_name("path")
                    .map(|p| self.rust_receiver(p))
                    .unwrap_or_default();
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.rust_receiver(n))
                    .unwrap_or_default();
                format!("{path}::{name}")
            }
            "reference_type" => {
                let inner = node
                    .child_by_field_name("type")
                    .map(|t| self.rust_receiver(t))
                    .unwrap_or_default();
                format!("&{inner}")
            }
            "pointer_type" => {
                let inner = node
                    .child_by_field_name("type")
                    .map(|t| self.rust_receiver(t))
                    .unwrap_or_default();
                format!("*{inner}")
            }
            "array_type" => {
                let elem = node
                    .child_by_field_name("element")
                    .map(|t| self.rust_receiver(t))
                    .unwrap_or_default();
                format!("[{elem}]")
            }
            "tuple_type" => {
                let parts: Vec<String> = named_children(node)
                    .into_iter()
                    .map(|c| self.rust_receiver(c))
                    .collect();
                format!("({})", parts.join(", "))
            }
            "unit_type" => "()".to_string(),
            "dynamic_type" | "abstract_type" => node
                .child_by_field_name("trait")
                .map(|t| self.rust_receiver(t))
                .unwrap_or_default(),
            "function_type" => node
                .child_by_field_name("trait")
                .map(|t| self.rust_receiver(t))
                .unwrap_or_else(|| "fn".to_string()),
            "bracketed_type" => {
                let inner = node
                    .named_child(0)
                    .map(|t| self.rust_receiver(t))
                    .unwrap_or_default();
                format!("<{inner}>")
            }
            "qualified_type" => {
                let ty = node
                    .child_by_field_name("type")
                    .map(|t| self.rust_receiver(t))
                    .unwrap_or_default();
                let alias = node
                    .child_by_field_name("alias")
                    .map(|a| self.rust_receiver(a))
                    .unwrap_or_default();
                format!("{ty} as {alias}")
            }
            "bounded_type" => {
                let parts: Vec<String> = named_children(node)
                    .into_iter()
                    .filter(|c| !matches!(c.kind(), "lifetime" | "use_bounds"))
                    .map(|c| self.rust_receiver(c))
                    .collect();
                parts.join(" + ")
            }
            "never_type" => "!".to_string(),
            _ => "_".to_string(),
        }
    }

    fn code_declaration(&mut self, node: Node<'_>, span: Node<'_>) {
        let kind = node.kind();
        if matches!(
            kind,
            "export_statement" | "ambient_declaration" | "decorated_definition"
        ) {
            for child in named_children(node) {
                if child.kind() != "decorator" {
                    self.code_declaration(child, span);
                }
            }
            return;
        }

        match kind {
            "import_statement" | "import_from_statement" => {
                if self.format.language == Some("python") {
                    for child in named_children(node) {
                        if matches!(child.kind(), "dotted_name" | "aliased_import") {
                            if let Some(module) = node.child_by_field_name("module_name")
                                && module.start_byte() == child.start_byte()
                            {
                                continue;
                            }
                            let name = child
                                .child_by_field_name("alias")
                                .or_else(|| child.named_child(0));
                            self.add_node("import", node, node, name, "");
                        }
                    }
                } else {
                    let mut names = Vec::new();
                    for child in named_children(node) {
                        if child.kind() == "import_clause" {
                            collect_js_import_names(child, &mut names);
                        }
                    }
                    for name in &names {
                        self.add_node("import", node, span, Some(*name), "");
                    }
                    if names.is_empty()
                        && let Some(src) = node.child_by_field_name("source")
                        && let Some(decoded) = document::decode_js_string_parts(self.text(src))
                    {
                        self.add(
                            "import",
                            decoded.display,
                            span.start_byte(),
                            span.end_byte(),
                            None,
                            None,
                            !node.has_error(),
                        );
                        if let Some(last) = self.entries.last_mut() {
                            last.name_raw_json = decoded.raw_json;
                        }
                    }
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                let entry_kind = if node.child(0).is_some_and(|c| c.kind() == "const") {
                    "constant"
                } else {
                    "variable"
                };
                for decl in named_children(node) {
                    if decl.kind() == "variable_declarator" {
                        let mut bindings = Vec::new();
                        if let Some(name) = decl.child_by_field_name("name") {
                            collect_binding_nodes(name, &mut bindings);
                        }
                        for name in bindings {
                            self.add_node(entry_kind, node, span, Some(name), "");
                        }
                    }
                }
            }
            "function_declaration"
            | "generator_function_declaration"
            | "function_signature"
            | "function_definition" => {
                self.add_node("function", node, span, node.child_by_field_name("name"), "");
            }
            "class_declaration" | "abstract_class_declaration" | "class_definition" => {
                let name = node.child_by_field_name("name");
                self.add_node("class", node, span, name, "");
                let receiver = name.map(|n| self.text(n).to_string()).unwrap_or_default();
                if let Some(body) = node.child_by_field_name("body") {
                    for candidate in named_children(body) {
                        let method = if candidate.kind() == "decorated_definition" {
                            match candidate.child_by_field_name("definition") {
                                Some(def) => def,
                                None => continue,
                            }
                        } else {
                            candidate
                        };
                        if matches!(
                            method.kind(),
                            "method_definition"
                                | "method_signature"
                                | "abstract_method_signature"
                                | "function_definition"
                        ) {
                            self.add_node(
                                "method",
                                method,
                                candidate,
                                method.child_by_field_name("name"),
                                &receiver,
                            );
                        }
                    }
                }
            }
            "type_alias_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "type_alias_statement" => {
                self.add_node("type", node, span, node.child_by_field_name("name"), "");
            }
            "expression_statement" => {
                if self.format.language == Some("python") {
                    for child in named_children(node) {
                        self.python_assignment(child, span);
                    }
                }
            }
            "assignment" => {
                if self.format.language == Some("python") {
                    self.python_assignment(node, span);
                }
            }
            "ERROR" => {
                for child in named_children(node) {
                    self.code_declaration(child, child);
                }
            }
            _ => {}
        }
    }

    fn python_assignment(&mut self, node: Node<'_>, span: Node<'_>) {
        if node.kind() != "assignment" {
            return;
        }
        let mut bindings = Vec::new();
        if let Some(left) = node.child_by_field_name("left") {
            collect_binding_nodes(left, &mut bindings);
        }
        for name in bindings {
            self.add_node("variable", node, span, Some(name), "");
        }
        if let Some(right) = node.child_by_field_name("right") {
            self.python_assignment(right, span);
        }
    }

    fn visit_errors_and_tokens(&mut self, node: Node<'_>, data: &[u8], collect_tokens: bool) {
        if !collect_tokens && !node.has_error() && !node.is_error() && !node.is_missing() {
            return;
        }
        if node.is_error() || node.is_missing() {
            self.parse_error(node.start_byte());
        }
        let kind = node.kind();
        if collect_tokens
            && (matches!(
                kind,
                "identifier"
                    | "property_identifier"
                    | "private_property_identifier"
                    | "type_identifier"
                    | "shorthand_property_identifier"
                    | "shorthand_property_identifier_pattern"
            ) || (self.format.language == Some("rust") && kind == "field_identifier"))
            && let Ok(text) = node.utf8_text(data)
        {
            self.tokens.push(Token {
                from: node.start_byte(),
                text: text.to_string(),
            });
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit_errors_and_tokens(child, data, collect_tokens);
        }
    }
}

pub(crate) fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn collect_rust_error_peers<'a>(node: Node<'a>, depth: &mut usize, out: &mut Vec<Node<'a>>) {
    if node.is_error() {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_rust_error_peers(child, depth, out);
        }
        return;
    }
    match node.kind() {
        "{" => *depth += 1,
        "}" => *depth = depth.saturating_sub(1),
        _ => {
            if *depth == 0 && node.is_named() {
                out.push(node);
            }
        }
    }
}

fn collect_js_import_names<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    match node.kind() {
        "import_specifier" => {
            if let Some(name) = node
                .child_by_field_name("alias")
                .or_else(|| node.child_by_field_name("name"))
            {
                out.push(name);
            }
            return;
        }
        "namespace_import" => {
            for child in named_children(node) {
                if child.kind() == "identifier" {
                    out.push(child);
                }
            }
            return;
        }
        "identifier" => {
            out.push(node);
            return;
        }
        "string" => return,
        _ => {}
    }
    for child in named_children(node) {
        collect_js_import_names(child, out);
    }
}

fn collect_binding_nodes<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(node),
        "pair_pattern" => {
            if let Some(val) = node.child_by_field_name("value") {
                collect_binding_nodes(val, out);
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(left) = node.child_by_field_name("left") {
                collect_binding_nodes(left, out);
            }
        }
        "attribute" | "subscript" | "type" => {}
        "array_pattern"
        | "object_pattern"
        | "rest_pattern"
        | "list_pattern"
        | "tuple_pattern"
        | "pattern_list"
        | "list_splat_pattern"
        | "dictionary_splat_pattern" => {
            for child in named_children(node) {
                collect_binding_nodes(child, out);
            }
        }
        _ => {}
    }
}

/// Normalize BOM and bare CR into parser whitespace while preserving exact byte offsets.
pub(crate) fn normalize_parser_bytes(source: &str) -> Vec<u8> {
    let mut data = source.as_bytes().to_vec();
    if source.starts_with('\u{feff}') && data.len() >= 3 {
        data[0] = b' ';
        data[1] = b' ';
        data[2] = b' ';
    }
    let len = data.len();
    for i in 0..len {
        if data[i] == b'\r' && (i + 1 == len || data[i + 1] != b'\n') {
            data[i] = b'\n';
        }
    }
    data
}

pub(crate) fn parse_tree(data: &[u8], language: Language) -> Result<tree_sitter::Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|err| format!("configure parser: {err}"))?;
    parser
        .parse(data, None)
        .ok_or_else(|| "source parsing failed".to_string())
}

/// Parse source extracting both structural entries and lexical tokens.
pub fn parse(source: String, format: SourceFormat) -> Result<Source, String> {
    parse_internal(source, format, /*collect_tokens*/ true)
}

/// Parse source extracting structural outline entries without collecting tokens.
pub fn parse_outline(source: String, format: SourceFormat) -> Result<Source, String> {
    parse_internal(source, format, /*collect_tokens*/ false)
}

fn parse_internal(
    source: String,
    format: SourceFormat,
    collect_tokens: bool,
) -> Result<Source, String> {
    let mut doc = Source::new(source, format);
    if format.language == Some("go") {
        go::parse_go(&mut doc, collect_tokens)?;
    } else if format.kind == "markdown" {
        document::parse_markdown(&mut doc);
    } else if format.kind == "json" {
        document::parse_json(&mut doc, collect_tokens)?;
    } else {
        let language: Language = match format.language {
            Some("python") => tree_sitter_python::LANGUAGE.into(),
            Some("rust") => tree_sitter_rust::LANGUAGE.into(),
            Some("javascript") => tree_sitter_javascript::LANGUAGE.into(),
            Some("typescript") => {
                if format.jsx {
                    tree_sitter_typescript::LANGUAGE_TSX.into()
                } else {
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
                }
            }
            other => {
                return Err(format!("unsupported source language {other:?}"));
            }
        };
        let data = normalize_parser_bytes(&doc.source);
        let tree = parse_tree(&data, language)?;
        doc.code_outline(tree.root_node());
        doc.visit_errors_and_tokens(tree.root_node(), &data, collect_tokens);
    }

    doc.entries.sort_by_key(|entry| entry.from);
    Ok(doc)
}
