use crate::classifier::wrapper::DYNAMIC;
use crate::classifier::wrapper::basename;
use crate::classifier::wrapper::command_word;
use crate::protocol::VCS_TOOLS;
use crate::protocol::shell_single_quote;
use tree_sitter::Node;
use tree_sitter::Parser;

/// Resolves a Git alias using the invocation's global options.
/// Returns `None` when the name is not a configured alias.
pub type AliasLookup<'a> = &'a dyn Fn(&[String], &str) -> Option<String>;

pub fn is_vcs_tool(name: &str) -> bool {
    VCS_TOOLS.contains(&basename(name))
}

/// Reports whether `argv` (whose first element names the tool) may write to a
/// remote repository or hosting service.
pub fn writes(argv: &[String], lookup: Option<AliasLookup<'_>>) -> bool {
    writes_depth(argv, lookup, 0)
}

fn writes_depth(argv: &[String], lookup: Option<AliasLookup<'_>>, depth: usize) -> bool {
    if argv.is_empty() || depth > 8 {
        return depth > 8;
    }
    let args = &argv[1..];
    match basename(&argv[0]) {
        "git" => git_writes(args, lookup, depth),
        "gh" => gh_remote_write(args),
        "hg" => hg_writes(args),
        "svn" => svn_writes(args),
        "jj" => jj_writes(args),
        _ => false,
    }
}

fn vcs_value_option(tool: &str, option: &str) -> bool {
    match tool {
        "git" => matches!(
            option,
            "-C" | "-c"
                | "--git-dir"
                | "--work-tree"
                | "--namespace"
                | "--config-env"
                | "--super-prefix"
                | "--attr-source"
        ),
        "hg" => matches!(
            option,
            "-R" | "--repository"
                | "--repo"
                | "--cwd"
                | "--config"
                | "--encoding"
                | "--encodingmode"
                | "--color"
                | "--pager"
        ),
        "svn" => matches!(
            option,
            "--username" | "--password" | "--config-dir" | "--config-option"
        ),
        "jj" => matches!(
            option,
            "-R" | "--repository"
                | "--at-operation"
                | "--at-op"
                | "--color"
                | "--config"
                | "--config-toml"
                | "--config-file"
        ),
        _ => false,
    }
}

fn has_help_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}

fn git_invocation(args: &[String]) -> (Vec<String>, Option<&str>, &[String]) {
    let mut globals = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if vcs_value_option("git", arg) {
            if i + 1 == args.len() {
                return (globals, None, &[]);
            }
            globals.push(arg.clone());
            globals.push(args[i + 1].clone());
            i += 2;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        if matches!(
            arg.as_str(),
            "--help"
                | "-h"
                | "--version"
                | "-v"
                | "--exec-path"
                | "--html-path"
                | "--man-path"
                | "--info-path"
        ) {
            return (globals, None, &[]);
        }
        globals.push(arg.clone());
        i += 1;
    }
    if i == args.len() {
        return (globals, None, &[]);
    }
    (globals, Some(args[i].as_str()), &args[i + 1..])
}

/// Built-in Git commands that cannot be aliases and do not push on their own.
const GIT_COMMANDS: &[&str] = &[
    "add",
    "am",
    "annotate",
    "apply",
    "archive",
    "bisect",
    "blame",
    "branch",
    "bugreport",
    "bundle",
    "cat-file",
    "check-attr",
    "check-ignore",
    "check-mailmap",
    "check-ref-format",
    "checkout",
    "checkout-index",
    "cherry",
    "cherry-pick",
    "citool",
    "clean",
    "clone",
    "column",
    "commit",
    "commit-graph",
    "commit-tree",
    "config",
    "count-objects",
    "credential",
    "credential-cache",
    "credential-store",
    "describe",
    "diagnose",
    "diff",
    "diff-files",
    "diff-index",
    "diff-tree",
    "difftool",
    "fast-export",
    "fast-import",
    "fetch",
    "fetch-pack",
    "filter-branch",
    "fmt-merge-msg",
    "for-each-ref",
    "for-each-repo",
    "format-patch",
    "fsck",
    "fsck-objects",
    "gc",
    "get-tar-commit-id",
    "grep",
    "gui",
    "hash-object",
    "help",
    "hook",
    "index-pack",
    "init",
    "init-db",
    "instaweb",
    "interpret-trailers",
    "log",
    "ls-files",
    "ls-remote",
    "ls-tree",
    "mailinfo",
    "mailsplit",
    "maintenance",
    "merge",
    "merge-base",
    "merge-file",
    "merge-tree",
    "mergetool",
    "mktag",
    "mktree",
    "multi-pack-index",
    "mv",
    "name-rev",
    "notes",
    "pack-objects",
    "pack-redundant",
    "pack-refs",
    "patch-id",
    "prune",
    "prune-packed",
    "pull",
    "push",
    "range-diff",
    "read-tree",
    "rebase",
    "reflog",
    "refs",
    "remote",
    "repack",
    "replace",
    "replay",
    "request-pull",
    "rerere",
    "reset",
    "restore",
    "rev-list",
    "rev-parse",
    "revert",
    "rm",
    "send-email",
    "send-pack",
    "shortlog",
    "show",
    "show-branch",
    "show-index",
    "show-ref",
    "sparse-checkout",
    "stage",
    "stash",
    "status",
    "stripspace",
    "submodule",
    "subtree",
    "switch",
    "symbolic-ref",
    "tag",
    "unpack-file",
    "unpack-objects",
    "update-index",
    "update-ref",
    "update-server-info",
    "var",
    "verify-commit",
    "verify-pack",
    "verify-tag",
    "version",
    "whatchanged",
    "worktree",
    "write-tree",
    "svn",
    "p4",
    "lfs",
];

fn dry_run(args: &[String]) -> bool {
    let mut dry = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" | "-n" => dry = true,
            "--no-dry-run" => dry = false,
            _ => {}
        }
    }
    dry
}

fn git_push_writes(args: &[String]) -> bool {
    let mut dry = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            break;
        }
        if arg.starts_with("--") {
            let (name, attached) = match arg.split_once('=') {
                Some((before, _)) => (before, true),
                None => (arg.as_str(), false),
            };
            if attached {
                i += 1;
                continue;
            }
            match name {
                "--help" => return false,
                "--dry-run" => dry = true,
                _ => {
                    if "--no-dry-run".starts_with(name) {
                        dry = false;
                    } else if [
                        "--push-option",
                        "--repo",
                        "--receive-pack",
                        "--exec",
                        "--recurse-submodules",
                    ]
                    .iter()
                    .any(|option| option.starts_with(name))
                    {
                        i += 1;
                    }
                }
            }
            i += 1;
            continue;
        }
        if !arg.starts_with('-') {
            i += 1;
            continue;
        }
        let bytes = arg.as_bytes();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'h' => return false,
                b'n' => dry = true,
                b'o' => {
                    if j + 1 == bytes.len() {
                        i += 1;
                    }
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        i += 1;
    }
    !dry
}

fn git_writes(args: &[String], lookup: Option<AliasLookup<'_>>, depth: usize) -> bool {
    if depth > 8 {
        return true;
    }
    let (globals, Some(command), rest) = git_invocation(args) else {
        return false;
    };
    if command == DYNAMIC {
        return true;
    }
    if command == "push" {
        return git_push_writes(rest);
    }
    if has_help_flag(rest) && !matches!(command, "submodule" | "rebase" | "bisect") {
        return false;
    }
    match command {
        "send-email" | "send-pack" => return !dry_run(rest),
        "subtree" => {
            return rest.iter().any(|arg| arg == "push" || arg == DYNAMIC);
        }
        "svn" => {
            return rest.first().is_some_and(|first| {
                matches!(
                    first.as_str(),
                    "dcommit" | "commit-diff" | "set-tree" | "branch" | "tag" | DYNAMIC
                )
            });
        }
        "p4" => {
            return rest
                .first()
                .is_some_and(|first| first == "submit" || first == DYNAMIC);
        }
        "lfs" => {
            return rest.first().is_some_and(|first| {
                matches!(first.as_str(), "push" | "lock" | "unlock" | DYNAMIC)
            });
        }
        "submodule" => {
            let Some(at) = rest.iter().position(|arg| arg == "foreach") else {
                return false;
            };
            let mut body = &rest[at + 1..];
            while let Some(first) = body.first() {
                if matches!(first.as_str(), "--recursive" | "-q" | "--quiet") {
                    body = &body[1..];
                } else {
                    break;
                }
            }
            return script_writes(&body.join(" "), lookup, depth + 1);
        }
        "rebase" => {
            for (j, arg) in rest.iter().enumerate() {
                let script = if let Some(s) = arg.strip_prefix("--exec=") {
                    Some(s)
                } else if (arg == "-x" || arg == "--exec") && j + 1 < rest.len() {
                    Some(rest[j + 1].as_str())
                } else if let Some(s) = arg.strip_prefix("-x")
                    && !s.is_empty()
                {
                    Some(s)
                } else {
                    None
                };
                if let Some(script) = script
                    && (script == DYNAMIC || script_writes(script, lookup, depth + 1))
                {
                    return true;
                }
            }
            return false;
        }
        "bisect" => {
            if rest.len() > 1 && rest[0] == "run" {
                let words = &rest[1..];
                if words[0] == DYNAMIC {
                    return true;
                }
                if let Some(unwrapped) = command_word(words)
                    && is_vcs_tool(&words[unwrapped.index])
                {
                    return writes_depth(&words[unwrapped.index..], lookup, depth + 2);
                }
            }
            return false;
        }
        _ => {}
    }

    if GIT_COMMANDS.contains(&command) {
        return false;
    }
    let Some(lookup_fn) = lookup else {
        return true;
    };
    let Some(expansion) = lookup_fn(&globals, command) else {
        return true; // External git subcommand may push unguarded via git exec-path.
    };
    if let Some(shell_script) = expansion.strip_prefix('!') {
        let mut script = shell_script.to_owned();
        for arg in rest {
            script.push(' ');
            script.push_str(&quote_alias_arg(arg));
        }
        return script_writes(&script, lookup, depth + 1);
    }
    let Some(words) = split_alias(&expansion) else {
        return true;
    };
    if words.is_empty() {
        return true;
    }
    let mut expanded = Vec::with_capacity(globals.len() + words.len() + rest.len());
    expanded.extend(globals);
    expanded.extend(words);
    expanded.extend_from_slice(rest);
    git_writes(&expanded, lookup, depth + 1)
}

fn quote_alias_arg(word: &str) -> String {
    if word == DYNAMIC {
        "\"$1\"".to_owned()
    } else {
        shell_single_quote(word)
    }
}

/// Splits a non-shell Git alias as Git's `split_cmdline` does.
pub(crate) fn split_alias(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in text.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
            in_word = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            in_word = true;
        } else if matches!(ch, ' ' | '\t' | '\n') {
            if in_word {
                words.push(std::mem::take(&mut word));
                in_word = false;
            }
        } else {
            word.push(ch);
            in_word = true;
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// Parses an embedded shell script (from a Git shell alias, `submodule foreach`,
/// or `rebase --exec`) with `tree-sitter-bash` and reports whether it invokes a
/// guarded VCS tool for a remote write. Fails closed on parse errors.
fn script_writes(script: &str, lookup: Option<AliasLookup<'_>>, depth: usize) -> bool {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return true;
    }
    let Some(tree) = parser.parse(script, None) else {
        return true;
    };
    if tree.root_node().has_error() {
        return true;
    }
    let mut found = false;
    walk_script_commands(tree.root_node(), script, lookup, depth, &mut found);
    found
}

fn walk_script_commands(
    node: Node<'_>,
    src: &str,
    lookup: Option<AliasLookup<'_>>,
    depth: usize,
    found: &mut bool,
) {
    if *found {
        return;
    }
    if node.kind() == "command" {
        let mut raw_words = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "variable_assignment"
                | "file_redirect"
                | "heredoc_redirect"
                | "herestring_redirect"
                | "comment" => continue,
                "command_name" => {
                    if let Some(word_node) = child.named_child(0) {
                        raw_words.push(node_literal(word_node, src));
                    } else {
                        raw_words.push(node_literal(child, src));
                    }
                }
                _ if child.is_named() => {
                    raw_words.push(node_literal(child, src));
                }
                _ => {}
            }
        }
        if let Some(unwrapped) = command_word(&raw_words)
            && is_vcs_tool(&raw_words[unwrapped.index])
            && writes_depth(&raw_words[unwrapped.index..], lookup, depth + 1)
        {
            *found = true;
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_script_commands(child, src, lookup, depth, found);
        if *found {
            return;
        }
    }
}

/// Extracts the static literal value of a `tree-sitter-bash` word node, or returns
/// `DYNAMIC` if the word contains any runtime expansion.
pub(crate) fn node_literal(node: Node<'_>, src: &str) -> String {
    match static_word_value(node, src) {
        Some(value) => value,
        None => DYNAMIC.to_owned(),
    }
}

/// Unquotes a `tree-sitter-bash` word node if it contains only literal/quoted text
/// without expansions or tilde expansion.
pub(crate) fn static_word_value(node: Node<'_>, src: &str) -> Option<String> {
    let text = node.utf8_text(src.as_bytes()).ok()?;
    match node.kind() {
        "word" | "number" => {
            if text.starts_with('~') {
                return None;
            }
            Some(unescape_bare_word(text))
        }
        "raw_string" => {
            let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
            Some(inner.to_owned())
        }
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
                let part = static_word_value(child, src)?;
                if first && child.kind() == "word" && part.starts_with('~') {
                    return None;
                }
                first = false;
                out.push_str(&part);
            }
            Some(out)
        }
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

const GH_LOCAL: &[&str] = &[
    "alias",
    "auth",
    "config",
    "completion",
    "help",
    "version",
    "status",
    "search",
    "browse",
];

const GH_READS: &[&str] = &[
    "list",
    "ls",
    "view",
    "status",
    "diff",
    "checks",
    "checkout",
    "co",
    "clone",
    "download",
    "watch",
    "verify",
    "verify-asset",
    "get",
    "check",
    "field-list",
    "item-list",
    "logs",
    "trusted-root",
    "search",
    "set-default",
];

fn positionals(args: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--" {
            words.extend(args[i + 1..].iter().map(String::as_str));
            return words;
        }
        if !arg.starts_with('-') || arg == "-" {
            words.push(arg.as_str());
        }
    }
    words
}

fn gh_remote_write(args: &[String]) -> bool {
    if args.is_empty() || matches!(args[0].as_str(), "--version" | "--help" | "-h") {
        return false;
    }
    let words = positionals(args);
    if words.is_empty() {
        return false;
    }
    if GH_LOCAL.contains(&words[0]) {
        return false;
    }
    if words[0] == "api" {
        if let Some(pos) = args.iter().position(|arg| arg == "api") {
            return gh_api_writes(&args[pos + 1..]);
        }
        return true;
    }
    if has_help_flag(args) {
        return false;
    }
    if matches!(words[0], "extension" | "extensions" | "ext") {
        return words.len() < 2 || !matches!(words[1], "list" | "ls" | "search" | "browse");
    }
    if words.len() < 2 {
        return true;
    }
    let mut verb = words[1];
    if matches!(
        verb,
        "deploy-key" | "autolink" | "gitignore" | "license" | "ports"
    ) && words.len() > 2
    {
        verb = words[2];
    }
    if verb == "clone" {
        return words[0] != "repo" && words[0] != "gist";
    }
    !GH_READS.contains(&verb)
}

fn gh_api_writes(args: &[String]) -> bool {
    let mut method = String::new();
    let mut parameters = false;
    let mut fields = Vec::new();
    let mut endpoint = "";
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        let mut value = "";
        let mut has_value = false;
        let mut name = arg.as_str();
        if let Some((before, after)) = arg.split_once('=')
            && arg.starts_with("--")
        {
            name = before;
            value = after;
            has_value = true;
        } else if arg.len() > 2
            && arg.starts_with('-')
            && !arg.starts_with("--")
            && matches!(
                arg.as_bytes()[1],
                b'X' | b'f' | b'F' | b'H' | b'q' | b't' | b'p'
            )
        {
            name = &arg[..2];
            value = &arg[2..];
            has_value = true;
        }
        let takes = matches!(
            name,
            "-X" | "--method"
                | "-f"
                | "--raw-field"
                | "-F"
                | "--field"
                | "--input"
                | "-H"
                | "--header"
                | "-q"
                | "--jq"
                | "-t"
                | "--template"
                | "-p"
                | "--preview"
                | "--hostname"
                | "--cache"
        );
        if takes && !has_value {
            if i + 1 == args.len() {
                return true;
            }
            value = args[i + 1].as_str();
            i += 1;
        }
        match name {
            "-X" | "--method" => method = value.to_ascii_uppercase(),
            "-f" | "--raw-field" | "-F" | "--field" => {
                parameters = true;
                fields.push(value.to_owned());
            }
            "--input" => {
                parameters = true;
                fields.push(format!("@{value}"));
            }
            _ => {
                if !takes && !arg.starts_with('-') && endpoint.is_empty() {
                    endpoint = arg.as_str();
                }
            }
        }
        i += 1;
    }

    if endpoint.is_empty() || endpoint == DYNAMIC || method == DYNAMIC {
        return true;
    }
    if endpoint == "graphql" || endpoint == "/graphql" {
        if !method.is_empty() && method != "POST" && method != "GET" {
            return true;
        }
        for field in &fields {
            let value = field.split_once('=').map_or("", |(_, v)| v);
            if field.starts_with('@')
                || value.starts_with('@')
                || field.contains(DYNAMIC)
                || value.to_ascii_lowercase().contains("mutation")
            {
                return true;
            }
        }
        return false;
    }
    if method.is_empty() {
        if parameters {
            return true;
        }
        method.push_str("GET");
    }
    method != "GET" && method != "HEAD"
}

fn hg_writes(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if vcs_value_option("hg", arg) {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        if arg == DYNAMIC {
            return true;
        }
        if has_help_flag(&args[i + 1..]) {
            return false;
        }
        for (name, prefix) in [("push", 3), ("email", 2), ("phabsend", 5)] {
            if arg.len() >= prefix && name.starts_with(arg.as_str()) {
                return true;
            }
        }
        return false;
    }
    false
}

const SVN_URL_WRITES: &[&str] = &[
    "copy", "cp", "move", "mv", "rename", "ren", "delete", "del", "remove", "rm", "mkdir",
    "propset", "pset", "ps", "propdel", "pdel", "pd", "propedit", "pedit", "pe",
];

fn svn_writes(args: &[String]) -> bool {
    let mut command = "";
    let mut at = 0;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if vcs_value_option("svn", arg) {
            i += 2;
            continue;
        }
        if !arg.starts_with('-') {
            command = arg.as_str();
            at = i;
            break;
        }
        i += 1;
    }
    if command.is_empty() {
        return false;
    }
    let rest = &args[at + 1..];
    if has_help_flag(rest) {
        return false;
    }
    if command == DYNAMIC || matches!(command, "commit" | "ci" | "import" | "lock" | "unlock") {
        return true;
    }
    if SVN_URL_WRITES.contains(&command) {
        return rest.iter().any(|arg| {
            arg == "--revprop" || arg == DYNAMIC || arg.contains("://") || arg.starts_with("^/")
        });
    }
    false
}

const JJ_COMMANDS: &[&str] = &[
    "abandon",
    "absorb",
    "backout",
    "bisect",
    "bookmark",
    "b",
    "branch",
    "cat",
    "checkout",
    "chmod",
    "commit",
    "ci",
    "config",
    "debug",
    "describe",
    "desc",
    "diff",
    "diffedit",
    "duplicate",
    "edit",
    "evolog",
    "obslog",
    "file",
    "files",
    "fix",
    "help",
    "interdiff",
    "log",
    "merge",
    "metaedit",
    "move",
    "new",
    "next",
    "operation",
    "op",
    "parallelize",
    "prev",
    "rebase",
    "redo",
    "resolve",
    "restore",
    "revert",
    "root",
    "run",
    "show",
    "sign",
    "simplify-parents",
    "sparse",
    "split",
    "squash",
    "st",
    "status",
    "tag",
    "undo",
    "unsign",
    "untrack",
    "util",
    "version",
    "workspace",
];

fn jj_writes(args: &[String]) -> bool {
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if vcs_value_option("jj", arg) {
            i += 2;
            continue;
        }
        if matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V") {
            return false;
        }
        if !arg.starts_with('-') {
            words.push(arg.as_str());
        }
        i += 1;
    }
    if words.is_empty() {
        return false;
    }
    let has_dry_run = args.iter().any(|arg| arg == "--dry-run");
    match words[0] {
        "git" => {
            return words.get(1).is_some_and(|w| *w == "push" || *w == DYNAMIC) && !has_dry_run;
        }
        "gerrit" => {
            return words
                .get(1)
                .is_some_and(|w| *w == "upload" || *w == DYNAMIC)
                && !has_dry_run;
        }
        _ => {}
    }
    !JJ_COMMANDS.contains(&words[0])
}
