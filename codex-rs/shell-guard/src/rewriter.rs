use crate::classifier::remote::static_word_value;
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

/// Rewrites only executable words that `PATH` shims alone cannot intercept:
/// explicit paths (e.g. `/usr/bin/git`, `$tools/sudo`), `env -i` / `env PATH=...` /
/// `env -u PATH`, `command -p`, `PATH=...` overrides, and nested `-c` shells.
///
/// Bare guarded commands (`git push`, `sudo id`, `gpg --sign`) in a normal environment
/// are left completely untouched so `PATH` shims handle them with zero AST rewriting.
pub fn rewrite_script(script: &str, config: &RewriteConfig<'_>) -> String {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return script.to_owned();
    }
    let Some(tree) = parser.parse(script, None) else {
        return script.to_owned();
    };
    let root = tree.root_node();
    let script_mutates_path = config.force_rewrite_bare || has_script_path_mutation(root, script);
    let mut edits = Vec::new();
    collect_edits(root, script, config, script_mutates_path, &mut edits);
    apply_inserts(script, edits)
}

fn has_script_path_mutation(node: Node<'_>, src: &str) -> bool {
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
        if has_script_path_mutation(child, src) {
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
    edits: &mut Vec<SourceInsert>,
) {
    if node.kind() == "command" {
        let mut word_nodes = Vec::new();
        let mut word_values = Vec::new();
        let mut local_path_override = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "variable_assignment" => {
                    if let Some(name) = child.child_by_field_name("name")
                        && name.utf8_text(src.as_bytes()).ok() == Some("PATH")
                    {
                        local_path_override = true;
                    }
                }
                "file_redirect" | "heredoc_redirect" | "herestring_redirect" | "comment" => {}
                "command_name" => {
                    let target = child.named_child(0).unwrap_or(child);
                    word_nodes.push(target);
                    word_values.push(word_for_wrapper(target, src));
                }
                _ if child.is_named() => {
                    word_nodes.push(child);
                    word_values.push(word_for_wrapper(child, src));
                }
                _ => {}
            }
        }

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
                if needs_rewrite {
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
        collect_edits(child, src, config, script_mutates_path, edits);
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
