use crate::classifier::wrapper::DYNAMIC;
use crate::classifier::wrapper::basename;
use crate::classifier::wrapper::command_word;
use crate::protocol::ARG1_GPG_COMMAND;
use crate::protocol::ARG1_GPG_COMMAND_DEFAULT;
use crate::protocol::ARG1_SHELL;
use crate::protocol::ARG1_SHELL_DEFAULT;
use crate::protocol::ARG1_SUDO_COMMAND;
use crate::protocol::ARG1_SUDO_COMMAND_DEFAULT;
use crate::protocol::ARG1_VCS_COMMAND;
use crate::protocol::ARG1_VCS_COMMAND_DEFAULT;
use crate::protocol::SHELLS;
use crate::protocol::VCS_TOOLS;
use crate::protocol::shell_single_quote;
use std::ffi::OsStr;
use std::path::Path;
use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Tree;

/// Configuration for the minimal `tree-sitter-bash` executable-word rewriter.
#[derive(Debug, Clone)]
pub struct RewriteConfig<'a> {
    /// Absolute path to the pinned `mekugi-app-server` binary.
    pub helper_exe: &'a Path,
    /// Session-private Unix socket path.
    pub socket_path: &'a Path,
    /// Per-session `vcs-guard` directory path (if available).
    pub vcs_dir: Option<&'a Path>,
    /// Per-session `sudo-guard` directory path (if available).
    pub sudo_dir: Option<&'a Path>,
    /// Thread ID to carry through environment-clearing wrappers.
    pub thread_id: &'a str,
    /// Tool call / item ID to carry through environment-clearing wrappers.
    pub item_id: &'a str,
    /// When true (e.g., inside `env -i sh -c '...'`), also rewrite bare guarded commands
    /// because the nested environment no longer has guard directories on `PATH`.
    pub force_rewrite_bare: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Vcs,
    Sudo,
    Gpg,
    Shell,
}

impl TargetKind {
    fn flag(self, default_path: bool) -> &'static str {
        match (self, default_path) {
            (Self::Vcs, false) => ARG1_VCS_COMMAND,
            (Self::Vcs, true) => ARG1_VCS_COMMAND_DEFAULT,
            (Self::Sudo, false) => ARG1_SUDO_COMMAND,
            (Self::Sudo, true) => ARG1_SUDO_COMMAND_DEFAULT,
            (Self::Gpg, false) => ARG1_GPG_COMMAND,
            (Self::Gpg, true) => ARG1_GPG_COMMAND_DEFAULT,
            (Self::Shell, false) => ARG1_SHELL,
            (Self::Shell, true) => ARG1_SHELL_DEFAULT,
        }
    }
}

#[derive(Debug, Clone)]
struct SourceInsert {
    offset: usize,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuotedHeredocSpan {
    redir_start: usize,
    redir_end: usize,
    body_start: usize,
    full_end: usize,
}

/// Parses a shell script with `tree-sitter-bash`, returning `None` if the grammar
/// cannot be initialized or parsing fails.
pub fn parse_bash(script: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()?;
    parser.parse(script, None)
}

/// Rewrites only executable words that `PATH` shims alone cannot intercept:
/// explicit paths (e.g. `/usr/bin/git`, `$tools/sudo`), `env -i` / `env PATH=...` /
/// `env -u PATH`, `command -p`, `PATH=...` overrides, and nested `-c` shells.
///
/// Bare guarded commands (`git push`, `sudo id`, `gpg --sign`) in a normal environment
/// are left completely untouched so `PATH` shims handle them with zero AST rewriting.
pub fn rewrite_script(script: &str, config: &RewriteConfig<'_>) -> String {
    let mut current = script.to_owned();
    let mut all_spans = Vec::new();
    let mut final_tree = None;

    for _ in 0..64 {
        let Some(tree) = parse_bash(&current) else {
            return script.to_owned();
        };
        let new_spans = quoted_heredoc_spans_from_tree(&current, &tree);
        if new_spans.is_empty() {
            final_tree = Some(tree);
            break;
        }
        current = mask_quoted_heredocs(&current, &new_spans);
        all_spans.extend(new_spans);
    }

    let Some(tree) = final_tree.or_else(|| parse_bash(&current)) else {
        return script.to_owned();
    };
    rewrite_script_tree_with_spans(script, &tree, config, &all_spans)
}

/// Rewrites executable words in an already-parsed tree-sitter-bash syntax tree.
pub fn rewrite_script_tree(script: &str, tree: &Tree, config: &RewriteConfig<'_>) -> String {
    let quoted_spans = quoted_heredoc_spans_from_tree(script, tree);
    rewrite_script_tree_with_spans(script, tree, config, &quoted_spans)
}

fn rewrite_script_tree_with_spans(
    script: &str,
    tree: &Tree,
    config: &RewriteConfig<'_>,
    quoted_spans: &[QuotedHeredocSpan],
) -> String {
    let root = tree.root_node();
    let script_mutates_path =
        config.force_rewrite_bare || has_script_path_mutation(root, script, quoted_spans);
    let mut edits = Vec::new();
    collect_edits(
        root,
        script,
        config,
        script_mutates_path,
        quoted_spans,
        &mut edits,
    );
    apply_inserts(script, edits)
}

/// Returns the word nodes of a `tree-sitter-bash` `command` node, skipping
/// variable assignments, redirects, and comments, and unwrapping `command_name`.
pub fn command_word_nodes<'a>(command_node: Node<'a>) -> Vec<Node<'a>> {
    let mut words = Vec::new();
    let mut cursor = command_node.walk();
    for child in command_node.children(&mut cursor) {
        match child.kind() {
            "variable_assignment"
            | "file_redirect"
            | "heredoc_redirect"
            | "herestring_redirect"
            | "comment" => {}
            "command_name" => words.push(child.named_child(0).unwrap_or(child)),
            _ if child.is_named() => words.push(child),
            _ => {}
        }
    }
    words
}

/// Unquotes a `tree-sitter-bash` word node if it contains only literal/quoted text
/// without expansions or tilde expansion.
pub fn static_word_value(node: Node<'_>, src: &str) -> Option<String> {
    static_word_value_with_expansion(node, src, |_, _| None)
}

/// Unquote a word, accepting only expansions explicitly supplied by the caller.
pub fn static_word_value_with_expansion(
    node: Node<'_>,
    src: &str,
    resolve: impl Fn(&str, bool) -> Option<String> + Copy,
) -> Option<String> {
    let text = node.utf8_text(src.as_bytes()).ok()?;
    match node.kind() {
        "word" | "number" => (!text.starts_with('~')).then(|| unescape_bare_word(text)),
        "raw_string" => text
            .strip_prefix('\'')?
            .strip_suffix('\'')
            .map(ToOwned::to_owned),
        "string" => {
            let mut cursor = node.walk();
            let mut out = String::new();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "\"" => {}
                    "string_content" => {
                        let content = child.utf8_text(src.as_bytes()).ok()?;
                        out.push_str(&unescape_double_quoted(content));
                    }
                    "simple_expansion" | "expansion" => {
                        out.push_str(&resolve(child.utf8_text(src.as_bytes()).ok()?, true)?);
                    }
                    _ => return None,
                }
            }
            Some(out)
        }
        "concatenation" => {
            let mut cursor = node.walk();
            let mut out = String::new();
            let mut first = true;
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                let part = static_word_value_with_expansion(child, src, resolve)?;
                if first && child.kind() == "word" && part.starts_with('~') {
                    return None;
                }
                first = false;
                out.push_str(&part);
            }
            Some(out)
        }
        "simple_expansion" | "expansion" => resolve(text, false),
        _ => None,
    }
}

fn unescape_bare_word(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next()
                && next != '\n'
            {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn unescape_double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some(next @ ('$' | '`' | '"' | '\\')) => out.push(next),
                Some('\n') => {}
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}
fn has_script_path_mutation(node: Node<'_>, src: &str, quoted_spans: &[QuotedHeredocSpan]) -> bool {
    if is_offset_in_quoted_heredoc(node.start_byte(), quoted_spans) {
        return false;
    }
    match node.kind() {
        "variable_assignment" => {
            if let Some(name) = node.child_by_field_name("name")
                && name.utf8_text(src.as_bytes()).ok() == Some("PATH")
            {
                return true;
            }
        }
        "unset_command" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() && child.utf8_text(src.as_bytes()).ok() == Some("PATH") {
                    return true;
                }
            }
        }
        "command" => {
            let mut cmd_name = None;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "command_name" {
                    cmd_name = child.utf8_text(src.as_bytes()).ok();
                } else if matches!(
                    cmd_name,
                    Some("unset" | "export" | "declare" | "local" | "typeset")
                ) && child.is_named()
                    && let Ok(text) = child.utf8_text(src.as_bytes())
                    && (text == "PATH" || text.starts_with("PATH="))
                {
                    return true;
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_script_path_mutation(child, src, quoted_spans) {
            return true;
        }
    }
    false
}

fn collect_edits(
    node: Node<'_>,
    src: &str,
    config: &RewriteConfig<'_>,
    script_mutates_path: bool,
    quoted_spans: &[QuotedHeredocSpan],
    edits: &mut Vec<SourceInsert>,
) {
    if is_offset_in_quoted_heredoc(node.start_byte(), quoted_spans) {
        return;
    }
    if node.kind() == "command" {
        let mut cursor = node.walk();
        let local_path_override = node.children(&mut cursor).any(|child| {
            child.kind() == "variable_assignment"
                && child
                    .child_by_field_name("name")
                    .and_then(|name| name.utf8_text(src.as_bytes()).ok())
                    == Some("PATH")
        });
        let word_nodes = command_word_nodes(node);
        let word_values: Vec<String> = word_nodes
            .iter()
            .map(|&word| word_for_wrapper(word, src))
            .collect();

        if let Some(unwrapped) = command_word(&word_values) {
            let word_node = word_nodes[unwrapped.index];
            if let Some((kind, bare)) = classify_target(word_node, src) {
                let needs_rewrite = match kind {
                    TargetKind::Shell => {
                        shell_payload(&word_values[unwrapped.index + 1..]).is_some()
                    }
                    TargetKind::Vcs | TargetKind::Sudo | TargetKind::Gpg => {
                        !bare
                            || unwrapped.default_path
                            || unwrapped.env_cleared_path
                            || local_path_override
                            || script_mutates_path
                    }
                };
                if needs_rewrite
                    && !is_offset_in_quoted_heredoc(word_node.start_byte(), quoted_spans)
                {
                    let prefix = build_helper_prefix(kind, unwrapped.default_path, config);
                    edits.push(SourceInsert {
                        offset: word_node.start_byte(),
                        text: prefix,
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_edits(child, src, config, script_mutates_path, quoted_spans, edits);
    }
}

fn is_offset_in_quoted_heredoc(offset: usize, spans: &[QuotedHeredocSpan]) -> bool {
    spans
        .iter()
        .any(|span| offset >= span.body_start && offset < span.full_end)
}

/// Replaces non-newline bytes of quoted heredoc redirections, bodies, and
/// terminating delimiter lines with ASCII spaces while preserving exact byte
/// offsets (and backslash-newline continuations inside redirections) so
/// tree-sitter-bash neither misparses literal heredoc bodies as commands nor
/// loses track of commands that follow a heredoc after list/pipeline operators.
fn mask_quoted_heredocs(script: &str, spans: &[QuotedHeredocSpan]) -> String {
    let mut bytes = script.as_bytes().to_vec();
    for span in spans {
        let mut idx = span.redir_start;
        while idx < span.redir_end {
            if bytes[idx] == b'\\' && bytes.get(idx + 1) == Some(&b'\n') {
                idx += 2;
            } else if bytes[idx] == b'\\'
                && bytes.get(idx + 1) == Some(&b'\r')
                && bytes.get(idx + 2) == Some(&b'\n')
            {
                idx += 3;
            } else if bytes[idx] == b'\n' || bytes[idx] == b'\r' {
                idx += 1;
            } else {
                bytes[idx] = b' ';
                idx += 1;
            }
        }
        for byte in &mut bytes[span.body_start..span.full_end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| script.to_owned())
}

#[derive(Debug, Clone)]
struct HeredocEntry {
    redir_start: usize,
    redir_end: usize,
    delimiter: String,
    strip_tabs: bool,
    is_quoted: bool,
    absorbed_redirect: bool,
}

/// Uses tree-sitter-bash's own heredoc_start AST nodes to locate quoted
/// here-document spans without re-implementing a full Bash lexer.
fn quoted_heredoc_spans_from_tree(src: &str, tree: &Tree) -> Vec<QuotedHeredocSpan> {
    let mut start_nodes = Vec::new();
    let mut error_nodes = Vec::new();
    collect_heredoc_candidates(tree.root_node(), &mut start_nodes, &mut error_nodes);
    if start_nodes.is_empty() && error_nodes.is_empty() {
        return Vec::new();
    }
    start_nodes.sort_by_key(Node::start_byte);

    // Group heredoc_start nodes by their logical command line (body_start).
    let mut line_groups: Vec<(usize, Vec<HeredocEntry>)> = Vec::new();
    let mut push_entry = |body_start: usize, entry: HeredocEntry| {
        if let Some((_, entries)) = line_groups.iter_mut().find(|(b, _)| *b == body_start) {
            if !entries.iter().any(|e| e.redir_start == entry.redir_start) {
                entries.push(entry);
            }
        } else {
            line_groups.push((body_start, vec![entry]));
        }
    };

    for node in start_nodes {
        if node.parent().is_some_and(|p| p.kind() == "ERROR") {
            continue;
        }
        let Some((redir_start, strip_tabs)) = find_heredoc_operator_before(src, node.start_byte())
        else {
            continue;
        };
        let word_end = scan_shell_word_end(src.as_bytes(), node.start_byte(), src.len());
        let word_text = &src[node.start_byte()..word_end];
        let Some((delimiter, is_quoted)) = parse_heredoc_word(word_text) else {
            continue;
        };
        let body_start = find_heredoc_body_start(src, tree, word_end);
        push_entry(
            body_start,
            HeredocEntry {
                redir_start,
                redir_end: word_end,
                delimiter,
                strip_tabs,
                is_quoted,
                absorbed_redirect: node.end_byte() > word_end,
            },
        );
    }

    // Recover heredoc redirects that tree-sitter-bash placed inside ERROR nodes
    // (e.g. prefix FD redirects like 3<<'EOF' cmd or mixed multi-heredoc lines).
    for err in error_nodes {
        if inside_non_heredoc_context(err) {
            continue;
        }
        let scan_start = err
            .parent()
            .filter(|p| p.kind() == "file_redirect")
            .map_or(err.start_byte(), |p| p.start_byte());
        let scan_end = find_heredoc_body_start(src, tree, err.start_byte());
        for entry in scan_error_line_heredocs(src, scan_start, scan_end) {
            let body_start = find_heredoc_body_start(src, tree, entry.redir_end);
            push_entry(body_start, entry);
        }
    }

    let mut spans = Vec::new();
    for (body_start, mut entries) in line_groups {
        if !entries.iter().any(|e| e.is_quoted) {
            continue;
        }
        entries.sort_by_key(|e| e.redir_start);
        let mut pos = body_start;
        for entry in entries {
            let entry_body_start = pos;
            let (full_end, terminated) = advance_past_heredoc_body(src, &mut pos, &entry);
            let mask_unquoted_redirect_bug = !entry.is_quoted
                && entry.absorbed_redirect
                && terminated
                && !src.as_bytes()[entry_body_start..full_end]
                    .iter()
                    .any(|&b| matches!(b, b'$' | b'\x60'));
            if entry.is_quoted || mask_unquoted_redirect_bug {
                spans.push(QuotedHeredocSpan {
                    redir_start: entry.redir_start,
                    redir_end: entry.redir_end,
                    body_start: entry_body_start,
                    full_end,
                });
            }
        }
    }
    spans
}

fn collect_heredoc_candidates<'a>(
    node: Node<'a>,
    starts: &mut Vec<Node<'a>>,
    errors: &mut Vec<Node<'a>>,
) {
    match node.kind() {
        "heredoc_start" => {
            starts.push(node);
            return;
        }
        "ERROR" => {
            errors.push(node);
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_heredoc_candidates(child, starts, errors);
    }
}

fn inside_non_heredoc_context(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if matches!(
            parent.kind(),
            "command_substitution" | "process_substitution"
        ) {
            return false;
        }
        if matches!(
            parent.kind(),
            "expansion"
                | "arithmetic_expansion"
                | "string"
                | "raw_string"
                | "ansi_c_string"
                | "heredoc_body"
        ) {
            return true;
        }
        node = parent;
    }
    false
}

/// Finds the << or <<- operator (and optional leading FD digits) immediately
/// preceding a heredoc_start node.
fn find_heredoc_operator_before(src: &str, word_start: usize) -> Option<(usize, bool)> {
    let bytes = src.as_bytes();
    let mut i = word_start;
    while i > 0 {
        if bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' {
            i -= 1;
        } else if i >= 2 && bytes[i - 1] == b'\n' && bytes[i - 2] == b'\\' {
            i -= 2;
        } else if i >= 3 && bytes[i - 1] == b'\n' && bytes[i - 2] == b'\r' && bytes[i - 3] == b'\\'
        {
            i -= 3;
        } else {
            break;
        }
    }
    let strip_tabs = i > 0 && bytes[i - 1] == b'-';
    if strip_tabs {
        i -= 1;
    }
    if i < 2 || bytes[i - 1] != b'<' || bytes[i - 2] != b'<' {
        return None;
    }
    let op_start = i - 2;
    if op_start > 0 && bytes[op_start - 1] == b'<' {
        return None;
    }
    Some((leading_fd_start(bytes, op_start), strip_tabs))
}

fn leading_fd_start(bytes: &[u8], op_start: usize) -> usize {
    let mut start = op_start;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == op_start {
        return op_start;
    }
    let at_boundary = start == 0
        || matches!(
            bytes[start - 1],
            b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
        );
    if at_boundary { start } else { op_start }
}

/// Finds the byte offset immediately after the newline that terminates the
/// logical command line containing heredoc_start, skipping backslash-newline
/// continuations and multiline strings or command substitutions on that line.
fn find_heredoc_body_start(src: &str, tree: &Tree, after_word: usize) -> usize {
    let bytes = src.as_bytes();
    let mut pos = after_word;
    while pos < bytes.len() {
        let Some(rel_nl) = bytes[pos..].iter().position(|&b| b == b'\n') else {
            return bytes.len();
        };
        let nl = pos + rel_nl;
        if is_line_continuation(bytes, tree, nl) {
            pos = nl + 1;
            continue;
        }
        if let Some(skip_end) = enclosing_multiline_construct_end(tree, after_word, nl) {
            pos = skip_end.max(nl + 1);
            continue;
        }
        return nl + 1;
    }
    bytes.len()
}

fn is_line_continuation(bytes: &[u8], tree: &Tree, nl: usize) -> bool {
    let prev = if nl > 0 && bytes[nl - 1] == b'\r' {
        nl - 1
    } else {
        nl
    };
    if prev == 0 || bytes[prev - 1] != b'\\' {
        return false;
    }
    if let Some(mut node) = tree
        .root_node()
        .descendant_for_byte_range(prev - 1, prev - 1)
    {
        loop {
            if node.kind() == "comment" {
                return false;
            }
            let Some(parent) = node.parent() else {
                break;
            };
            node = parent;
        }
    }
    let trailing_backslashes = bytes[..prev]
        .iter()
        .rev()
        .take_while(|&&b| b == b'\\')
        .count();
    trailing_backslashes % 2 == 1
}

fn enclosing_multiline_construct_end(tree: &Tree, after_word: usize, nl: usize) -> Option<usize> {
    let mut node = tree.root_node().descendant_for_byte_range(nl, nl)?;
    loop {
        if matches!(
            node.kind(),
            "command_substitution"
                | "process_substitution"
                | "string"
                | "raw_string"
                | "ansi_c_string"
        ) && node.start_byte() >= after_word
            && node.end_byte() > nl
        {
            return Some(node.end_byte());
        }
        node = node.parent()?;
    }
}

fn scan_error_line_heredocs(src: &str, start: usize, end: usize) -> Vec<HeredocEntry> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'\\' => i = (i + 2).min(end),
            b'\'' => {
                i += 1;
                while i < end && bytes[i] != b'\'' {
                    i += 1;
                }
                i = (i + 1).min(end);
            }
            b'"' => {
                i += 1;
                while i < end {
                    match bytes[i] {
                        b'\\' => i = (i + 2).min(end),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'<' if i + 1 < end && bytes[i + 1] == b'<' => {
                if i + 2 < end && bytes[i + 2] == b'<' {
                    i += 3;
                    continue;
                }
                let redir_start = leading_fd_start(bytes, i);
                i += 2;
                let strip_tabs = i < end && bytes[i] == b'-';
                if strip_tabs {
                    i += 1;
                }
                while i < end && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                let word_start = i;
                i = scan_shell_word_end(bytes, word_start, end);
                if word_start < i
                    && let Some((delimiter, is_quoted)) = parse_heredoc_word(&src[word_start..i])
                {
                    out.push(HeredocEntry {
                        redir_start,
                        redir_end: i,
                        delimiter,
                        strip_tabs,
                        is_quoted,
                        absorbed_redirect: matches!(bytes.get(i), Some(b'>' | b'<')),
                    });
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn scan_shell_word_end(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    while i < limit {
        if matches!(bytes[i], b'<' | b'>' | b'$') && bytes.get(i + 1) == Some(&b'(') {
            i = skip_balanced_parens(bytes, i + 2, limit);
            continue;
        }
        if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'{') {
            i = skip_balanced_param_exp(bytes, i + 2, limit);
            continue;
        }
        if bytes[i] == b'`' {
            i = skip_backtick(bytes, i + 1, limit);
            continue;
        }
        if matches!(
            bytes[i],
            b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
        ) {
            break;
        }
        i = skip_word_atom(bytes, i, limit);
    }
    i
}

fn skip_word_atom(bytes: &[u8], i: usize, limit: usize) -> usize {
    match bytes[i] {
        b'\\' => (i + 2).min(limit),
        b'\'' => skip_single_quoted(bytes, i + 1, limit),
        b'"' => skip_double_quoted(bytes, i + 1, limit),
        b'$' if bytes.get(i + 1) == Some(&b'\'') => skip_ansi_c_quoted(bytes, i + 2, limit),
        _ => i + 1,
    }
}

fn skip_single_quoted(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    while i < limit && bytes[i] != b'\'' {
        i += 1;
    }
    (i + 1).min(limit)
}

fn skip_ansi_c_quoted(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    while i < limit {
        match bytes[i] {
            b'\\' => i = (i + 2).min(limit),
            b'\'' => return i + 1,
            _ => i += 1,
        }
    }
    limit
}

fn skip_double_quoted(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    while i < limit {
        match bytes[i] {
            b'\\' => i = (i + 2).min(limit),
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    limit
}

fn skip_backtick(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    while i < limit {
        match bytes[i] {
            b'\\' => i = (i + 2).min(limit),
            b'`' => return i + 1,
            _ => i += 1,
        }
    }
    limit
}

fn skip_balanced_parens(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    let mut depth = 1usize;
    while i < limit && depth > 0 {
        if matches!(bytes[i], b'<' | b'>' | b'$') && bytes.get(i + 1) == Some(&b'(') {
            i = skip_balanced_parens(bytes, i + 2, limit);
        } else if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'{') {
            i = skip_balanced_param_exp(bytes, i + 2, limit);
        } else if bytes[i] == b'`' {
            i = skip_backtick(bytes, i + 1, limit);
        } else {
            match bytes[i] {
                b'(' => {
                    depth += 1;
                    i += 1;
                }
                b')' => {
                    depth -= 1;
                    i += 1;
                }
                _ => i = skip_word_atom(bytes, i, limit),
            }
        }
    }
    i
}

fn skip_balanced_param_exp(bytes: &[u8], mut i: usize, limit: usize) -> usize {
    let mut depth = 1usize;
    while i < limit && depth > 0 {
        if matches!(bytes[i], b'<' | b'>' | b'$') && bytes.get(i + 1) == Some(&b'(') {
            i = skip_balanced_parens(bytes, i + 2, limit);
        } else if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'{') {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'`' {
            i = skip_backtick(bytes, i + 1, limit);
        } else if bytes[i] == b'}' {
            depth -= 1;
            i += 1;
        } else {
            i = skip_word_atom(bytes, i, limit);
        }
    }
    i
}

fn advance_past_heredoc_body(src: &str, pos: &mut usize, entry: &HeredocEntry) -> (usize, bool) {
    let bytes = src.as_bytes();
    while *pos < bytes.len() {
        let line_start = *pos;
        let rel_nl = bytes[line_start..].iter().position(|&b| b == b'\n');
        let (line_end, next_pos) = match rel_nl {
            Some(offset) => (line_start + offset, line_start + offset + 1),
            None => (src.len(), src.len()),
        };
        let raw_line = &src[line_start..line_end];
        let candidate = if entry.strip_tabs {
            raw_line.trim_start_matches('\t')
        } else {
            raw_line
        };
        *pos = next_pos;
        if candidate == entry.delimiter {
            return (next_pos, true);
        }
    }
    (src.len(), false)
}

/// Decodes a heredoc delimiter word and returns (delimiter, is_quoted).
fn parse_heredoc_word(word: &str) -> Option<(String, bool)> {
    let bytes = word.as_bytes();
    let mut pos = 0;
    let mut out = String::new();
    let mut is_quoted = false;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\'' => {
                is_quoted = true;
                pos += 1;
                let start = pos;
                while pos < bytes.len() && bytes[pos] != b'\'' {
                    pos += 1;
                }
                out.push_str(&word[start..pos]);
                pos = (pos + 1).min(bytes.len());
            }
            b'"' => {
                is_quoted = true;
                pos += 1;
                parse_double_quoted_delimiter(word, &mut pos, &mut out);
            }
            b'$' if bytes.get(pos + 1) == Some(&b'\'') => {
                is_quoted = true;
                pos += 2;
                if !parse_ansi_c_delimiter(word, &mut pos, &mut out) {
                    return None;
                }
            }
            b'$' if bytes.get(pos + 1) == Some(&b'"') => {
                is_quoted = true;
                pos += 2;
                parse_double_quoted_delimiter(word, &mut pos, &mut out);
            }
            b'\\' => {
                if bytes.get(pos + 1) == Some(&b'\n') {
                    pos += 2;
                    continue;
                }
                if bytes.get(pos + 1) == Some(&b'\r') && bytes.get(pos + 2) == Some(&b'\n') {
                    pos += 3;
                    continue;
                }
                is_quoted = true;
                pos += 1;
                if let Some(ch) = word[pos..].chars().next() {
                    out.push(ch);
                    pos += ch.len_utf8();
                }
            }
            _ => {
                if let Some(ch) = word[pos..].chars().next() {
                    out.push(ch);
                    pos += ch.len_utf8();
                }
            }
        }
    }
    Some((out, is_quoted))
}

fn parse_double_quoted_delimiter(src: &str, pos: &mut usize, out: &mut String) {
    let bytes = src.as_bytes();
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'"' => {
                *pos += 1;
                break;
            }
            b'\\' => {
                *pos += 1;
                if *pos >= bytes.len() {
                    out.push('\\');
                    break;
                }
                match bytes[*pos] {
                    b'\n' => *pos += 1,
                    b'\r' if bytes.get(*pos + 1) == Some(&b'\n') => *pos += 2,
                    b'"' | b'\\' | b'$' | b'`' => {
                        out.push(bytes[*pos] as char);
                        *pos += 1;
                    }
                    _ => {
                        out.push('\\');
                        if let Some(ch) = src[*pos..].chars().next() {
                            out.push(ch);
                            *pos += ch.len_utf8();
                        }
                    }
                }
            }
            _ => {
                if let Some(ch) = src[*pos..].chars().next() {
                    out.push(ch);
                    *pos += ch.len_utf8();
                }
            }
        }
    }
}

fn parse_ansi_c_delimiter(src: &str, pos: &mut usize, out: &mut String) -> bool {
    let bytes = src.as_bytes();
    let mut raw = Vec::new();
    let mut nul = false;
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'\'' => {
                *pos += 1;
                break;
            }
            b'\\' => {
                *pos += 1;
                let Some(&esc) = bytes.get(*pos) else {
                    if !nul {
                        raw.push(b'\\');
                    }
                    break;
                };
                match esc {
                    b'a' | b'b' | b'e' | b'E' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\'
                    | b'\'' | b'"' | b'?' => {
                        *pos += 1;
                        if !nul {
                            raw.push(match esc {
                                b'a' => 0x07,
                                b'b' => 0x08,
                                b'e' | b'E' => 0x1b,
                                b'f' => 0x0c,
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                b'v' => 0x0b,
                                other => other,
                            });
                        }
                    }
                    b'0'..=b'7' => {
                        let mut val = 0u32;
                        for _ in 0..3 {
                            let Some(&b @ b'0'..=b'7') = bytes.get(*pos) else {
                                break;
                            };
                            val = (val << 3) | u32::from(b - b'0');
                            *pos += 1;
                        }
                        push_ansi_byte(&mut raw, &mut nul, (val & 0xff) as u8);
                    }
                    b'x' => {
                        *pos += 1;
                        match read_hex(bytes, pos, 2) {
                            Some(val) => push_ansi_byte(&mut raw, &mut nul, (val & 0xff) as u8),
                            None if !nul => raw.extend_from_slice(b"\\x"),
                            None => {}
                        }
                    }
                    b'u' | b'U' => {
                        *pos += 1;
                        match read_hex(bytes, pos, if esc == b'u' { 4 } else { 8 }) {
                            Some(0) => nul = true,
                            Some(val) => {
                                let Some(ch) = char::from_u32(val) else {
                                    return false;
                                };
                                if !nul {
                                    let mut buf = [0u8; 4];
                                    raw.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                                }
                            }
                            None if !nul => raw.extend_from_slice(&[b'\\', esc]),
                            None => {}
                        }
                    }
                    b'c' => {
                        *pos += 1;
                        if let Some(&ctrl) = bytes.get(*pos) {
                            *pos += 1;
                            let byte = if ctrl == b'?' { 0x7f } else { ctrl & 0x1f };
                            push_ansi_byte(&mut raw, &mut nul, byte);
                        } else if !nul {
                            raw.extend_from_slice(b"\\c");
                        }
                    }
                    other => {
                        *pos += 1;
                        if !nul {
                            raw.push(b'\\');
                            raw.push(other);
                        }
                    }
                }
            }
            byte => {
                *pos += 1;
                if !nul {
                    raw.push(byte);
                }
            }
        }
    }
    let Ok(decoded) = String::from_utf8(raw) else {
        return false;
    };
    out.push_str(&decoded);
    true
}

fn push_ansi_byte(raw: &mut Vec<u8>, nul: &mut bool, byte: u8) {
    if byte == 0 {
        *nul = true;
    } else if !*nul {
        raw.push(byte);
    }
}

fn read_hex(bytes: &[u8], pos: &mut usize, max_digits: usize) -> Option<u32> {
    let mut val = 0u32;
    let mut count = 0;
    while count < max_digits {
        let digit = match bytes.get(*pos).copied() {
            Some(b @ b'0'..=b'9') => u32::from(b - b'0'),
            Some(b @ b'a'..=b'f') => u32::from(b - b'a' + 10),
            Some(b @ b'A'..=b'F') => u32::from(b - b'A' + 10),
            _ => break,
        };
        val = (val << 4) | digit;
        *pos += 1;
        count += 1;
    }
    (count > 0).then_some(val)
}

/// Locate the command string, not a script filename or its positional args.
/// At rewrite time a dynamic option may expand to `-c`; defer that decision
/// to the trampoline, which calls this again with expanded arguments.
pub(crate) fn shell_payload(args: &[impl AsRef<OsStr>]) -> Option<usize> {
    let mut command_string = false;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let flag = arg.as_ref().to_string_lossy();
        if flag.contains(DYNAMIC) {
            return Some(i);
        }
        if flag == "--" {
            return (command_string && i + 1 < args.len()).then_some(i + 1);
        }
        if flag == "-" || !flag.starts_with(['-', '+']) {
            return command_string.then_some(i);
        }
        if matches!(
            flag.as_ref(),
            "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file"
        ) {
            i += 2;
        } else {
            command_string |=
                flag.starts_with('-') && !flag.starts_with("--") && flag.contains('c');
            i += 1;
        }
    }
    None
}

fn word_for_wrapper(node: Node<'_>, src: &str) -> String {
    if let Some(value) = static_word_value(node, src) {
        return value;
    }
    let prefix = leading_literal_prefix(node, src);
    format!("{prefix}{DYNAMIC}")
}

fn leading_literal_prefix(node: Node<'_>, src: &str) -> String {
    match node.kind() {
        "word" | "number" => node
            .utf8_text(src.as_bytes())
            .map(ToOwned::to_owned)
            .unwrap_or_default(),
        "raw_string" => node
            .utf8_text(src.as_bytes())
            .ok()
            .and_then(|s| s.strip_prefix('\'')?.strip_suffix('\''))
            .map(ToOwned::to_owned)
            .unwrap_or_default(),
        "string" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "string_content" {
                    return child
                        .utf8_text(src.as_bytes())
                        .map(ToOwned::to_owned)
                        .unwrap_or_default();
                }
                if child.is_named() {
                    break;
                }
            }
            String::new()
        }
        "concatenation" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    return leading_literal_prefix(child, src);
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// Classifies a word node as a guarded tool or nested shell.
/// Returns `(TargetKind, is_bare)` where `is_bare` is true only for a static word
/// without `/` (e.g. `git`, not `/usr/bin/git` or `$tools/git`).
fn classify_target(node: Node<'_>, src: &str) -> Option<(TargetKind, bool)> {
    if let Some(value) = static_word_value(node, src) {
        let name = basename(&value);
        let is_bare = !value.contains('/');
        return match_tool_name(name, /*allow_shell*/ true).map(|kind| (kind, is_bare));
    }
    // Dynamic path suffix such as `$tools/git` or `"${dir}/sudo"`.
    if let Some(suffix) = trailing_literal_suffix(node, src)
        && suffix.contains('/')
    {
        let name = basename(&suffix);
        return match_tool_name(name, /*allow_shell*/ false).map(|kind| (kind, false));
    }
    None
}

fn match_tool_name(name: &str, allow_shell: bool) -> Option<TargetKind> {
    if VCS_TOOLS.contains(&name) {
        return Some(TargetKind::Vcs);
    }
    if name == "sudo" {
        return Some(TargetKind::Sudo);
    }
    if name == "gpg" || name == "gpg2" {
        return Some(TargetKind::Gpg);
    }
    if allow_shell && SHELLS.contains(&name) {
        return Some(TargetKind::Shell);
    }
    None
}

fn trailing_literal_suffix(node: Node<'_>, src: &str) -> Option<String> {
    match node.kind() {
        "word" => node.utf8_text(src.as_bytes()).ok().map(ToOwned::to_owned),
        "raw_string" => node
            .utf8_text(src.as_bytes())
            .ok()
            .and_then(|s| s.strip_prefix('\'')?.strip_suffix('\''))
            .map(ToOwned::to_owned),
        "string" => {
            let mut last_named = None;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    last_named = Some(child);
                }
            }
            let last = last_named?;
            if last.kind() == "string_content" {
                last.utf8_text(src.as_bytes()).ok().map(ToOwned::to_owned)
            } else {
                None
            }
        }
        "concatenation" => {
            let mut last_named = None;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    last_named = Some(child);
                }
            }
            trailing_literal_suffix(last_named?, src)
        }
        _ => None,
    }
}

fn build_helper_prefix(kind: TargetKind, default_path: bool, config: &RewriteConfig<'_>) -> String {
    let mut parts = vec![
        shell_single_quote(&config.helper_exe.to_string_lossy()),
        kind.flag(default_path).to_owned(),
        "--socket".to_owned(),
        shell_single_quote(&config.socket_path.to_string_lossy()),
    ];
    if let Some(vcs_dir) = config.vcs_dir
        && matches!(kind, TargetKind::Shell)
    {
        parts.push("--vcs-dir".to_owned());
        parts.push(shell_single_quote(&vcs_dir.to_string_lossy()));
    }
    if let Some(sudo_dir) = config.sudo_dir
        && matches!(kind, TargetKind::Sudo | TargetKind::Shell)
    {
        parts.push("--sudo-dir".to_owned());
        parts.push(shell_single_quote(&sudo_dir.to_string_lossy()));
    }
    if !config.thread_id.is_empty() {
        parts.push("--thread".to_owned());
        parts.push(shell_single_quote(config.thread_id));
    }
    if !config.item_id.is_empty() {
        parts.push("--item".to_owned());
        parts.push(shell_single_quote(config.item_id));
    }
    parts.push("--".to_owned());
    let mut joined = parts.join(" ");
    joined.push(' ');
    joined
}

fn apply_inserts(script: &str, mut inserts: Vec<SourceInsert>) -> String {
    if inserts.is_empty() {
        return script.to_owned();
    }
    inserts.sort_by_key(|insert| insert.offset);
    let mut out = String::with_capacity(script.len() + inserts.len() * 64);
    let mut pos = 0;
    let mut last_offset: Option<usize> = None;
    for insert in inserts {
        if insert.offset > script.len() || last_offset == Some(insert.offset) {
            continue;
        }
        out.push_str(&script[pos..insert.offset]);
        out.push_str(&insert.text);
        pos = insert.offset;
        last_offset = Some(insert.offset);
    }
    out.push_str(&script[pos..]);
    out
}
