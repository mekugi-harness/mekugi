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
use std::collections::VecDeque;
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

#[derive(Debug, Clone)]
struct PendingHeredoc {
    redir_start: usize,
    redir_end: usize,
    delimiter: String,
    strip_tabs: bool,
    is_quoted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexMode {
    Normal,
    DoubleQuote,
    CmdSubst { paren_depth: usize },
    Arith { paren_depth: usize },
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
    let quoted_spans = quoted_heredoc_spans(script);
    let masked = mask_quoted_heredocs(script, &quoted_spans);
    let parse_input = masked.as_deref().unwrap_or(script);
    let Some(tree) = parse_bash(parse_input) else {
        return script.to_owned();
    };
    rewrite_script_tree_with_spans(script, &tree, config, &quoted_spans)
}

/// Rewrites executable words in an already-parsed `tree-sitter-bash` syntax tree.
pub fn rewrite_script_tree(script: &str, tree: &Tree, config: &RewriteConfig<'_>) -> String {
    let quoted_spans = quoted_heredoc_spans(script);
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
/// offsets so `tree-sitter-bash` neither misparses literal heredoc bodies as
/// commands nor loses track of commands that follow a heredoc after list or
/// pipeline operators (`&&`, `||`, `|`, `|&`).
fn mask_quoted_heredocs(script: &str, spans: &[QuotedHeredocSpan]) -> Option<String> {
    if spans.is_empty() {
        return None;
    }
    let mut bytes = script.as_bytes().to_vec();
    for span in spans {
        for byte in &mut bytes[span.redir_start..span.redir_end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
        for byte in &mut bytes[span.body_start..span.full_end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).ok()
}

/// Scans POSIX/Bash lexical structure to find byte spans of all quoted
/// here-document bodies (including their terminating delimiter lines).
fn quoted_heredoc_spans(src: &str) -> Vec<QuotedHeredocSpan> {
    let bytes = src.as_bytes();
    let mut spans = Vec::new();
    let mut pending = VecDeque::new();
    let mut modes = vec![LexMode::Normal];
    let mut at_word_start = true;
    let mut i = 0;

    while i < bytes.len() {
        match *modes.last().unwrap_or(&LexMode::Normal) {
            LexMode::DoubleQuote => match bytes[i] {
                b'\\' => {
                    i = (i + 2).min(bytes.len());
                }
                b'"' => {
                    modes.pop();
                    i += 1;
                }
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    if bytes.get(i + 2) == Some(&b'(') {
                        modes.push(LexMode::Arith { paren_depth: 2 });
                        i += 3;
                    } else {
                        modes.push(LexMode::CmdSubst { paren_depth: 1 });
                        at_word_start = true;
                        i += 2;
                    }
                }
                _ => {
                    i += 1;
                }
            },
            LexMode::Arith { .. } => match bytes[i] {
                b'\\' => {
                    i = (i + 2).min(bytes.len());
                }
                b'\'' => {
                    i = skip_single_quoted(bytes, i + 1);
                }
                b'"' => {
                    modes.push(LexMode::DoubleQuote);
                    i += 1;
                }
                b'(' => {
                    if let Some(LexMode::Arith { paren_depth }) = modes.last_mut() {
                        *paren_depth += 1;
                    }
                    i += 1;
                }
                b')' => {
                    let pop = if let Some(LexMode::Arith { paren_depth }) = modes.last_mut() {
                        *paren_depth -= 1;
                        *paren_depth == 0
                    } else {
                        false
                    };
                    if pop {
                        modes.pop();
                    }
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            LexMode::Normal | LexMode::CmdSubst { .. } => match bytes[i] {
                b'\\' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 2;
                    } else if bytes.get(i + 1) == Some(&b'\r') && bytes.get(i + 2) == Some(&b'\n') {
                        i += 3;
                    } else {
                        i = (i + 2).min(bytes.len());
                        at_word_start = false;
                    }
                }
                b'\n' => {
                    i += 1;
                    at_word_start = true;
                    if !pending.is_empty() {
                        consume_pending_heredocs(src, &mut i, &mut pending, &mut spans);
                    }
                }
                b'#' if at_word_start => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    if bytes.get(i + 2) == Some(&b'(') {
                        modes.push(LexMode::Arith { paren_depth: 2 });
                        at_word_start = false;
                        i += 3;
                    } else {
                        modes.push(LexMode::CmdSubst { paren_depth: 1 });
                        at_word_start = true;
                        i += 2;
                    }
                }
                b'$' if bytes.get(i + 1) == Some(&b'\'') => {
                    i = skip_ansi_c_quoted(bytes, i + 2);
                    at_word_start = false;
                }
                b'$' if bytes.get(i + 1) == Some(&b'"') => {
                    modes.push(LexMode::DoubleQuote);
                    at_word_start = false;
                    i += 2;
                }
                b'\'' => {
                    i = skip_single_quoted(bytes, i + 1);
                    at_word_start = false;
                }
                b'"' => {
                    modes.push(LexMode::DoubleQuote);
                    at_word_start = false;
                    i += 1;
                }
                b'(' => {
                    if let Some(LexMode::CmdSubst { paren_depth }) = modes.last_mut() {
                        *paren_depth += 1;
                    }
                    at_word_start = true;
                    i += 1;
                }
                b')' => {
                    let pop = if let Some(LexMode::CmdSubst { paren_depth }) = modes.last_mut() {
                        *paren_depth -= 1;
                        *paren_depth == 0
                    } else {
                        false
                    };
                    if pop {
                        modes.pop();
                    }
                    at_word_start = true;
                    i += 1;
                }
                b'<' if bytes.get(i + 1) == Some(&b'<') => {
                    if bytes.get(i + 2) == Some(&b'<') {
                        i += 3;
                        at_word_start = true;
                    } else {
                        let redir_start = leading_fd_start(bytes, i);
                        i += 2;
                        let strip_tabs = bytes.get(i) == Some(&b'-');
                        if strip_tabs {
                            i += 1;
                        }
                        if let Some(heredoc) =
                            parse_heredoc_delimiter(src, &mut i, redir_start, strip_tabs)
                        {
                            pending.push_back(heredoc);
                        }
                        at_word_start = false;
                    }
                }
                b' ' | b'\t' | b'\r' | b';' | b'&' | b'|' | b'<' | b'>' => {
                    at_word_start = true;
                    i += 1;
                }
                _ => {
                    at_word_start = false;
                    i += 1;
                }
            },
        }
    }

    spans
}

fn skip_single_quoted(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if byte == b'\'' {
            break;
        }
    }
    i
}

fn skip_ansi_c_quoted(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => return i + 1,
            _ => i += 1,
        }
    }
    i
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

fn parse_heredoc_delimiter(
    src: &str,
    pos: &mut usize,
    redir_start: usize,
    strip_tabs: bool,
) -> Option<PendingHeredoc> {
    let bytes = src.as_bytes();
    while *pos < bytes.len() {
        match bytes[*pos] {
            b' ' | b'\t' => *pos += 1,
            b'\\' if bytes.get(*pos + 1) == Some(&b'\n') => *pos += 2,
            b'\\' if bytes.get(*pos + 1) == Some(&b'\r') && bytes.get(*pos + 2) == Some(&b'\n') => {
                *pos += 3;
            }
            _ => break,
        }
    }

    let mut delimiter = String::new();
    let mut is_quoted = false;
    let mut saw_any = false;

    while *pos < bytes.len() {
        match bytes[*pos] {
            b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>' => {
                break;
            }
            b'\'' => {
                is_quoted = true;
                saw_any = true;
                *pos += 1;
                let start = *pos;
                while *pos < bytes.len() && bytes[*pos] != b'\'' {
                    *pos += 1;
                }
                delimiter.push_str(&src[start..*pos]);
                if *pos < bytes.len() {
                    *pos += 1;
                }
            }
            b'"' => {
                is_quoted = true;
                saw_any = true;
                *pos += 1;
                parse_double_quoted_delimiter(src, pos, &mut delimiter);
            }
            b'$' if bytes.get(*pos + 1) == Some(&b'\'') => {
                is_quoted = true;
                saw_any = true;
                *pos += 2;
                parse_ansi_c_delimiter(src, pos, &mut delimiter);
            }
            b'$' if bytes.get(*pos + 1) == Some(&b'"') => {
                is_quoted = true;
                saw_any = true;
                *pos += 2;
                parse_double_quoted_delimiter(src, pos, &mut delimiter);
            }
            b'\\' => {
                if bytes.get(*pos + 1) == Some(&b'\n') {
                    *pos += 2;
                    continue;
                }
                if bytes.get(*pos + 1) == Some(&b'\r') && bytes.get(*pos + 2) == Some(&b'\n') {
                    *pos += 3;
                    continue;
                }
                is_quoted = true;
                saw_any = true;
                *pos += 1;
                if let Some(ch) = src[*pos..].chars().next() {
                    delimiter.push(ch);
                    *pos += ch.len_utf8();
                }
            }
            _ => {
                saw_any = true;
                if let Some(ch) = src[*pos..].chars().next() {
                    delimiter.push(ch);
                    *pos += ch.len_utf8();
                }
            }
        }
    }

    saw_any.then_some(PendingHeredoc {
        redir_start,
        redir_end: *pos,
        delimiter,
        strip_tabs,
        is_quoted,
    })
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
                    b'\n' => {
                        *pos += 1;
                    }
                    b'\r' if bytes.get(*pos + 1) == Some(&b'\n') => {
                        *pos += 2;
                    }
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

fn parse_ansi_c_delimiter(src: &str, pos: &mut usize, out: &mut String) {
    let bytes = src.as_bytes();
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'\'' => {
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
                    b'\\' => {
                        out.push('\\');
                        *pos += 1;
                    }
                    b'\'' => {
                        out.push('\'');
                        *pos += 1;
                    }
                    b'n' => {
                        out.push('\n');
                        *pos += 1;
                    }
                    b'r' => {
                        out.push('\r');
                        *pos += 1;
                    }
                    b't' => {
                        out.push('\t');
                        *pos += 1;
                    }
                    _ => {
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

fn consume_pending_heredocs(
    src: &str,
    pos: &mut usize,
    pending: &mut VecDeque<PendingHeredoc>,
    spans: &mut Vec<QuotedHeredocSpan>,
) {
    if !pending.iter().any(|heredoc| heredoc.is_quoted) {
        pending.clear();
        return;
    }
    let bytes = src.as_bytes();
    while let Some(heredoc) = pending.pop_front() {
        let body_start = *pos;
        let mut full_end = src.len();

        while *pos < bytes.len() {
            let line_start = *pos;
            let rel_nl = bytes[line_start..].iter().position(|&b| b == b'\n');
            let (line_end, next_pos) = match rel_nl {
                Some(offset) => (line_start + offset, line_start + offset + 1),
                None => (src.len(), src.len()),
            };
            let raw_line = &src[line_start..line_end];
            let candidate = if heredoc.strip_tabs {
                raw_line.trim_start_matches('\t')
            } else {
                raw_line
            };
            *pos = next_pos;
            if candidate == heredoc.delimiter {
                full_end = next_pos;
                break;
            }
        }

        if heredoc.is_quoted {
            spans.push(QuotedHeredocSpan {
                redir_start: heredoc.redir_start,
                redir_end: heredoc.redir_end,
                body_start,
                full_end,
            });
        }
    }
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
