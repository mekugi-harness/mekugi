//! Fail-closed tree-sitter-bash AST span selection and eligibility gate.

use codex_shell_guard::command_word_nodes;
use codex_shell_guard::parse_bash;
use codex_shell_guard::static_word_value;
use tree_sitter::Node;

use crate::rewrite::rewrite_segment_boundaries;

/// Target shell dialect for AST eligibility checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellDialect {
    Bash,
    Dash,
}

/// One authored top-level segment selected from a shell script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentSpan {
    /// Zero-based segment index in source order.
    pub index: usize,
    /// Start byte offset in the authored script for boundary wrapping.
    pub start: usize,
    /// End byte offset in the authored script for boundary wrapping (before any
    /// trailing heredoc body).
    pub end: usize,
    /// Original authored source text (including any heredoc body).
    pub source: String,
    /// True when the segment contains a heredoc body and needs a newline
    /// separator before the closing boundary hook.
    pub has_heredoc: bool,
}

/// Result of splitting a script into trackable segments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitOutcome {
    pub segments: Vec<SegmentSpan>,
    pub shell_setup_collapsed: bool,
}

const INTERNAL_PREFIX: &str = "__mekugi_";

const UNTRACKED_BUILTINS: &[&str] = &[
    "wait", "jobs", "fg", "bg", "disown", "trap", "coproc", "suspend", "caller",
];

const UNTRACKED_PARAMETERS: &[&str] = &[
    "!",
    "PIPESTATUS",
    "_",
    "LINENO",
    "BASH_LINENO",
    "BASH_COMMAND",
    "BASH_SOURCE",
    "FUNCNAME",
    "BASH_EXECUTION_STRING",
];

const SHELL_SETUP_BUILTINS: &[&str] = &[
    "export", "readonly", "declare", "typeset", "local", "set", "shopt", "trap", "source", ".",
    "unset", "alias", "unalias", "umask",
];

/// Split script into authored segment spans when every segment can be tracked
/// inside one real shell without altering command semantics.
///
/// Returns None (fail-closed) on syntax errors, unsupported syntax, or
/// shell-state hazards (job control, traps, top-level returns, exec
/// replacement, xtrace/verbose tracing, or instrumentation-sensitive variables).
pub fn split_segments(script: &str, dialect: ShellDialect) -> Option<SplitOutcome> {
    if script.trim().is_empty() || script.contains(INTERNAL_PREFIX) {
        return None;
    }

    let tree = parse_bash(script)?;
    let root = tree.root_node();
    if root.has_error() || !is_trackable(root, script, dialect, /*in_function*/ false) {
        return None;
    }

    if !has_named_statements(root) {
        return None;
    }

    if has_shell_setup_node(root, script) {
        return Some(SplitOutcome {
            segments: vec![SegmentSpan {
                index: 0,
                start: 0,
                end: script.len(),
                source: script.trim_end_matches(['\r', '\n']).to_string(),
                has_heredoc: earliest_heredoc_body_start(root).is_some(),
            }],
            shell_setup_collapsed: true,
        });
    }

    let mut segments = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "comment" {
            continue;
        }
        collect_segments(child, script, &mut segments);
    }

    if segments.is_empty() {
        return None;
    }

    if segments.len() > 1 {
        let rewritten = rewrite_segment_boundaries(script, &segments);
        let reparsed = parse_bash(&rewritten)?;
        if reparsed.root_node().has_error() {
            return None;
        }
    }

    Some(SplitOutcome {
        segments,
        shell_setup_collapsed: false,
    })
}

/// Reports whether script mutates top-level shell state (e.g. export,
/// set, variable assignments, function definitions) and should stay collapsed
/// in a single segment.
pub fn has_shell_setup(script: &str) -> bool {
    let Some(tree) = parse_bash(script) else {
        return false;
    };
    let root = tree.root_node();
    if root.has_error() {
        return false;
    }
    has_shell_setup_node(root, script)
}

fn has_named_statements(root: Node<'_>) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .any(|child| child.kind() != "comment")
}

fn has_shell_setup_node(node: Node<'_>, script: &str) -> bool {
    match node.kind() {
        "subshell" | "command_substitution" | "process_substitution" | "pipeline" => return false,
        "function_definition" | "declaration_command" => return true,
        "variable_assignment" => {
            let is_command_local = node
                .parent()
                .is_some_and(|p| p.kind() == "command" && !command_word_nodes(p).is_empty());
            if !is_command_local {
                return true;
            }
        }
        "command" => {
            let words = command_word_nodes(node);
            if words.is_empty() {
                return has_variable_assignment_child(node);
            }
            let mut idx = 0;
            if let Some(first) = static_word_value(words[0], script)
                && matches!(first.as_str(), "builtin" | "command")
            {
                idx = 1;
            }
            if let Some(word) = words.get(idx)
                && let Some(name) = static_word_value(*word, script)
                && SHELL_SETUP_BUILTINS.contains(&name.as_str())
            {
                return true;
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if has_shell_setup_node(child, script) {
            return true;
        }
    }
    false
}

fn has_variable_assignment_child(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "variable_assignment")
}

fn collect_segments(node: Node<'_>, script: &str, out: &mut Vec<SegmentSpan>) {
    match node.kind() {
        "comment" => {}
        "list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_segments(child, script, out);
            }
        }
        "if_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    "elif_clause" | "else_clause" => {
                        let mut clause_cursor = child.walk();
                        for sub in child.named_children(&mut clause_cursor) {
                            collect_segments(sub, script, out);
                        }
                    }
                    _ => collect_segments(child, script, out),
                }
            }
        }
        _ => {
            let start = node.start_byte();
            let raw_end = node.end_byte();
            let has_heredoc = earliest_heredoc_body_start(node).is_some();
            let source = script[start..raw_end]
                .trim_end_matches(['\r', '\n'])
                .to_string();
            let index = out.len();
            out.push(SegmentSpan {
                index,
                start,
                end: raw_end,
                source,
                has_heredoc,
            });
        }
    }
}

fn earliest_heredoc_body_start(node: Node<'_>) -> Option<usize> {
    if node.kind() == "heredoc_body" {
        return Some(node.start_byte());
    }
    let mut earliest: Option<usize> = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(pos) = earliest_heredoc_body_start(child) {
            earliest = Some(earliest.map_or(pos, |cur| cur.min(pos)));
        }
    }
    earliest
}

fn is_trackable(node: Node<'_>, script: &str, dialect: ShellDialect, in_function: bool) -> bool {
    if node.is_error() || node.is_missing() {
        return false;
    }

    if !node.is_named() {
        let token = node.utf8_text(script.as_bytes()).unwrap_or("");
        if token == "&" {
            return false;
        }
        if dialect == ShellDialect::Dash && matches!(token, "|&" | "&>" | "&>>" | ";&" | ";;&") {
            return false;
        }
        return true;
    }

    if dialect == ShellDialect::Dash && is_bash_only_node(node, script) {
        return false;
    }

    match node.kind() {
        "function_definition" => {
            if let Some(body) = node.child_by_field_name("body") {
                return is_trackable(body, script, dialect, /*in_function*/ true);
            }
            return true;
        }
        "simple_expansion" | "expansion" => {
            if references_untracked_parameter(node, script) {
                return false;
            }
        }
        "command" => {
            let words = command_word_nodes(node);
            if let Some(first_word) = words.first()
                && let Some(name) = static_word_value(*first_word, script)
            {
                if UNTRACKED_BUILTINS.contains(&name.as_str()) {
                    return false;
                }
                if name == "return" && !in_function {
                    return false;
                }
                if name == "exec" && words.len() > 1 {
                    return false;
                }
                if matches!(name.as_str(), "set" | "shopt")
                    && has_tracing_option(&words[1..], script)
                {
                    return false;
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !is_trackable(child, script, dialect, in_function) {
            return false;
        }
    }
    true
}

fn is_bash_only_node(node: Node<'_>, script: &str) -> bool {
    match node.kind() {
        "test_command"
        | "c_style_for_statement"
        | "herestring_redirect"
        | "process_substitution"
        | "array"
        | "extglob_pattern" => true,
        "function_definition" => node
            .utf8_text(script.as_bytes())
            .is_ok_and(|text| text.trim_start().starts_with("function")),
        _ => false,
    }
}

fn references_untracked_parameter(node: Node<'_>, script: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "variable_name" | "special_variable_name")
            && let Ok(name) = child.utf8_text(script.as_bytes())
            && UNTRACKED_PARAMETERS.contains(&name)
        {
            return true;
        }
    }
    false
}

fn has_tracing_option(args: &[Node<'_>], script: &str) -> bool {
    for arg_node in args {
        let Some(arg) = static_word_value(*arg_node, script) else {
            return true;
        };
        if arg.contains("xtrace")
            || arg.contains("verbose")
            || arg.contains("extdebug")
            || arg.contains("functrace")
        {
            return true;
        }
        if (arg.starts_with('-') || arg.starts_with('+'))
            && !arg.starts_with("--")
            && arg[1..].chars().any(|ch| matches!(ch, 'x' | 'v' | 'T'))
        {
            return true;
        }
    }
    false
}
