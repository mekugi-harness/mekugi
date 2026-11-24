use crate::classifier::wrapper::DYNAMIC;
use crate::classifier::wrapper::basename;
use crate::classifier::wrapper::command_word;
use crate::protocol::VCS_TOOLS;
use crate::protocol::shell_single_quote;
use crate::rewriter::command_word_nodes;
use crate::rewriter::parse_bash;
use crate::rewriter::static_word_value;
use tree_sitter::Node;

const MAX_ALIAS_DEPTH: usize = 8;
const SAFE_INFO_FLAGS: &str = "--help -h --version -V";

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
    if argv.is_empty() || depth > MAX_ALIAS_DEPTH {
        return depth > MAX_ALIAS_DEPTH;
    }
    let args = &argv[1..];
    match basename(&argv[0]) {
        "git" => git_writes(args, lookup, depth),
        "gh" => gh_writes(args),
        "hg" => allowlist_writes(args, &HG_SPEC),
        "svn" => allowlist_writes(args, &SVN_SPEC),
        "jj" => jj_writes(args),
        _ => false,
    }
}

fn in_list(list: &str, item: &str) -> bool {
    list.split_ascii_whitespace().any(|word| word == item)
}

struct Invocation<'a> {
    globals: Vec<String>,
    command: &'a str,
    rest: &'a [String],
}

/// Scans global options up to the first positional subcommand, returning `None`
/// on help/version info flags, missing option operands, or no subcommand.
fn scan_invocation<'a>(
    args: &'a [String],
    value_opts: &str,
    safe_flags: &str,
) -> Option<Invocation<'a>> {
    let mut globals = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if in_list(value_opts, arg) {
            let value = args.get(i + 1)?;
            globals.push(arg.clone());
            globals.push(value.clone());
            i += 2;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            return Some(Invocation {
                globals,
                command: arg.as_str(),
                rest: &args[i + 1..],
            });
        }
        if in_list(safe_flags, arg) {
            return None;
        }
        globals.push(arg.clone());
        i += 1;
    }
    None
}

fn has_help_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}

fn first_positional<'a>(args: &'a [String], value_opts: &str) -> Option<&'a str> {
    scan_invocation(args, value_opts, "").map(|inv| inv.command)
}

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

// ---------------------------------------------------------------------------
// Declarative allowlist classifiers for hg, svn, and jj (fail-closed)
// ---------------------------------------------------------------------------

struct AllowlistSpec {
    value_opts: &'static str,
    safe_commands: &'static str,
    url_gated_commands: &'static str,
}

const HG_SPEC: AllowlistSpec = AllowlistSpec {
    value_opts: "-R --repository --repo --cwd --config --encoding --encodingmode --color --pager",
    safe_commands: "\
        add addremove annotate archive backout bisect blame bookmarks branch branches bundle cat \
        checkout ci clone co commit config copy cp diff export files forget graft grep heads help \
        history id identify import in incoming init locate log manifest merge mv out outgoing \
        parents paths pull rebase recover remove rename resolve revert rm root shelve showconfig \
        st status sum summary tag tags tip unbundle unshelve up update verify version",
    url_gated_commands: "",
};

const SVN_SPEC: AllowlistSpec = AllowlistSpec {
    value_opts: "--username --password --config-dir --config-option",
    safe_commands: "\
        add ann annotate auth blame cat changelist checkout cl cleanup co di diff export h help \
        info list log ls merge mergeinfo patch pg pget pl plist praise propget proplist relocate \
        resolve resolved revert st stat status sw switch up update upgrade ?",
    url_gated_commands: "\
        copy cp move mv rename ren delete del remove rm mkdir propset pset ps propdel pdel pd \
        propedit pedit pe",
};

fn allowlist_writes(args: &[String], spec: &AllowlistSpec) -> bool {
    let Some(inv) = scan_invocation(args, spec.value_opts, SAFE_INFO_FLAGS) else {
        return false;
    };
    if has_help_flag(inv.rest) || in_list(spec.safe_commands, inv.command) {
        return false;
    }
    if in_list(spec.url_gated_commands, inv.command) {
        return inv.rest.iter().any(|arg| {
            arg == "--revprop" || arg == DYNAMIC || arg.contains("://") || arg.starts_with("^/")
        });
    }
    true
}

const JJ_VALUE_OPTS: &str =
    "-R --repository --at-operation --at-op --color --config --config-toml --config-file";

const JJ_SAFE: &str = "\
    abandon absorb b backout bisect bookmark branch cat checkout chmod ci commit config debug \
    desc describe diff diffedit duplicate edit evolog file files fix help interdiff log merge \
    metaedit move new next obslog op operation parallelize prev rebase redo resolve restore \
    revert root run show sign simplify-parents sparse split squash st status tag undo unsign \
    untrack util version workspace";

const JJ_GIT_SAFE: &str = "clone colocation export fetch import init remote root";

fn jj_writes(args: &[String]) -> bool {
    if args.iter().any(|arg| in_list(SAFE_INFO_FLAGS, arg)) {
        return false;
    }
    let Some(inv) = scan_invocation(args, JJ_VALUE_OPTS, SAFE_INFO_FLAGS) else {
        return false;
    };
    let has_dry_run = args.iter().any(|arg| arg == "--dry-run");
    match inv.command {
        "git" => match first_positional(inv.rest, JJ_VALUE_OPTS) {
            None => false,
            Some(cmd) if in_list(JJ_GIT_SAFE, cmd) => false,
            Some("push") => !has_dry_run,
            Some(_) => true,
        },
        "gerrit" => match first_positional(inv.rest, JJ_VALUE_OPTS) {
            None => false,
            Some("upload") => !has_dry_run,
            Some(_) => true,
        },
        cmd => !in_list(JJ_SAFE, cmd),
    }
}

// ---------------------------------------------------------------------------
// Git classifier (built-ins, nested runners, alias expansion up to depth 8)
// ---------------------------------------------------------------------------

const GIT_VALUE_OPTS: &str =
    "-C -c --git-dir --work-tree --namespace --config-env --super-prefix --attr-source";
const GIT_SAFE_FLAGS: &str =
    "--help -h --version -v --exec-path --html-path --man-path --info-path";
const GIT_PUSH_VALUE_OPTS: &[&str] = &[
    "--push-option",
    "--repo",
    "--receive-pack",
    "--exec",
    "--recurse-submodules",
];

/// Built-in Git commands that cannot be aliases and do not push on their own.
const GIT_SAFE: &str = "\
    add am annotate apply archive blame branch bugreport bundle cat-file check-attr check-ignore \
    check-mailmap check-ref-format checkout checkout-index cherry cherry-pick citool clean clone \
    column commit commit-graph commit-tree config count-objects credential credential-cache \
    credential-store describe diagnose diff diff-files diff-index diff-tree difftool fast-export \
    fast-import fetch fetch-pack filter-branch fmt-merge-msg for-each-ref for-each-repo \
    format-patch fsck fsck-objects gc get-tar-commit-id grep gui hash-object help hook index-pack \
    init init-db instaweb interpret-trailers log ls-files ls-remote ls-tree mailinfo mailsplit \
    maintenance merge merge-base merge-file merge-tree mergetool mktag mktree multi-pack-index \
    mv name-rev notes pack-objects pack-redundant pack-refs patch-id prune prune-packed pull \
    range-diff read-tree reflog refs remote repack replace replay request-pull rerere reset \
    restore rev-list rev-parse revert rm shortlog show show-branch show-index show-ref \
    sparse-checkout stage stash status stripspace switch symbolic-ref tag unpack-file \
    unpack-objects update-index update-ref update-server-info var verify-commit verify-pack \
    verify-tag version whatchanged worktree write-tree";

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
            let (name, attached) = arg
                .split_once('=')
                .map_or((arg.as_str(), false), |(b, _)| (b, true));
            if !attached {
                match name {
                    "--help" => return false,
                    "--dry-run" => dry = true,
                    _ if "--no-dry-run".starts_with(name) => dry = false,
                    _ if GIT_PUSH_VALUE_OPTS.iter().any(|opt| opt.starts_with(name)) => i += 1,
                    _ => {}
                }
            }
        } else if arg.starts_with('-') {
            let bytes = arg.as_bytes();
            for j in 1..bytes.len() {
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
            }
        }
        i += 1;
    }
    !dry
}

fn git_writes(args: &[String], lookup: Option<AliasLookup<'_>>, depth: usize) -> bool {
    if depth > MAX_ALIAS_DEPTH {
        return true;
    }
    let Some(inv) = scan_invocation(args, GIT_VALUE_OPTS, GIT_SAFE_FLAGS) else {
        return false;
    };
    if inv.command == DYNAMIC {
        return true;
    }
    if inv.command == "push" {
        return git_push_writes(inv.rest);
    }
    if has_help_flag(inv.rest) && !matches!(inv.command, "submodule" | "rebase" | "bisect") {
        return false;
    }
    match inv.command {
        "send-email" | "send-pack" => return !dry_run(inv.rest),
        "subtree" => return inv.rest.iter().any(|arg| arg == "push" || arg == DYNAMIC),
        "svn" => {
            return inv.rest.first().is_some_and(|w| {
                matches!(
                    w.as_str(),
                    "dcommit" | "commit-diff" | "set-tree" | "branch" | "tag" | DYNAMIC
                )
            });
        }
        "p4" => {
            return inv
                .rest
                .first()
                .is_some_and(|w| w == "submit" || w == DYNAMIC);
        }
        "lfs" => {
            return inv
                .rest
                .first()
                .is_some_and(|w| matches!(w.as_str(), "push" | "lock" | "unlock" | DYNAMIC));
        }
        "submodule" => {
            let Some(at) = inv.rest.iter().position(|arg| arg == "foreach") else {
                return false;
            };
            let mut body = &inv.rest[at + 1..];
            while let Some(first) = body.first()
                && matches!(first.as_str(), "--recursive" | "-q" | "--quiet")
            {
                body = &body[1..];
            }
            return script_writes(&body.join(" "), lookup, depth + 1);
        }
        "rebase" => {
            for (j, arg) in inv.rest.iter().enumerate() {
                let script = arg
                    .strip_prefix("--exec=")
                    .or_else(|| {
                        ((arg == "-x" || arg == "--exec") && j + 1 < inv.rest.len())
                            .then(|| inv.rest[j + 1].as_str())
                    })
                    .or_else(|| arg.strip_prefix("-x").filter(|s| !s.is_empty()));
                if let Some(script) = script
                    && (script == DYNAMIC || script_writes(script, lookup, depth + 1))
                {
                    return true;
                }
            }
            return false;
        }
        "bisect" => {
            if inv.rest.len() > 1 && inv.rest[0] == "run" {
                let words = &inv.rest[1..];
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
        cmd if in_list(GIT_SAFE, cmd) => return false,
        _ => {}
    }

    let Some(expansion) = lookup.and_then(|f| f(&inv.globals, inv.command)) else {
        return true; // External git subcommand or unavailable alias lookup fails closed.
    };
    if let Some(shell_script) = expansion.strip_prefix('!') {
        let mut script = shell_script.to_owned();
        for arg in inv.rest {
            script.push(' ');
            if arg == DYNAMIC {
                script.push_str("\"$1\"");
            } else {
                script.push_str(&shell_single_quote(arg));
            }
        }
        return script_writes(&script, lookup, depth + 1);
    }
    let Some(words) = split_alias(&expansion) else {
        return true;
    };
    if words.is_empty() {
        return true;
    }
    let mut expanded = Vec::with_capacity(inv.globals.len() + words.len() + inv.rest.len());
    expanded.extend(inv.globals);
    expanded.extend(words);
    expanded.extend_from_slice(inv.rest);
    git_writes(&expanded, lookup, depth + 1)
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
    let Some(tree) = parse_bash(script) else {
        return true;
    };
    if tree.root_node().has_error() {
        return true;
    }
    walk_script_commands(tree.root_node(), script, lookup, depth)
}

fn walk_script_commands(
    node: Node<'_>,
    src: &str,
    lookup: Option<AliasLookup<'_>>,
    depth: usize,
) -> bool {
    if node.kind() == "command" {
        let raw_words: Vec<String> = command_word_nodes(node)
            .into_iter()
            .map(|w| static_word_value(w, src).unwrap_or_else(|| DYNAMIC.to_owned()))
            .collect();
        if let Some(unwrapped) = command_word(&raw_words)
            && is_vcs_tool(&raw_words[unwrapped.index])
            && writes_depth(&raw_words[unwrapped.index..], lookup, depth + 1)
        {
            return true;
        }
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| walk_script_commands(child, src, lookup, depth))
}

// ---------------------------------------------------------------------------
// GitHub CLI (gh) classifier (fail-closed)
// ---------------------------------------------------------------------------

const GH_LOCAL: &str = "alias auth browse completion config help search status version";
const GH_READS: &str = "\
    check checkout checks clone co diff download field-list get item-list list logs ls search \
    set-default status trusted-root verify verify-asset view watch";
const GH_API_VALUE_OPTS: &str = "\
    -X --method -f --raw-field -F --field --input -H --header -q --jq -t --template -p --preview \
    --hostname --cache";

fn gh_writes(args: &[String]) -> bool {
    if args.is_empty() || matches!(args[0].as_str(), "--version" | "--help" | "-h") {
        return false;
    }
    let words = positionals(args);
    let Some(&top) = words.first() else {
        return false;
    };
    if in_list(GH_LOCAL, top) {
        return false;
    }
    if top == "api" {
        return args
            .iter()
            .position(|arg| arg == "api")
            .is_none_or(|pos| gh_api_writes(&args[pos + 1..]));
    }
    if has_help_flag(args) {
        return false;
    }
    if matches!(top, "extension" | "extensions" | "ext") {
        return words
            .get(1)
            .is_none_or(|w| !matches!(*w, "list" | "ls" | "search" | "browse"));
    }
    let Some(&second) = words.get(1) else {
        return true;
    };
    let verb = if matches!(
        second,
        "deploy-key" | "autolink" | "gitignore" | "license" | "ports"
    ) && words.len() > 2
    {
        words[2]
    } else {
        second
    };
    if verb == "clone" {
        return top != "repo" && top != "gist";
    }
    !in_list(GH_READS, verb)
}

fn gh_api_writes(args: &[String]) -> bool {
    let mut method = String::new();
    let mut parameters = false;
    let mut fields = Vec::new();
    let mut endpoint = "";
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        let (name, attached) = if let Some((before, after)) = arg.split_once('=')
            && arg.starts_with("--")
        {
            (before, Some(after))
        } else if arg.len() > 2
            && arg.starts_with('-')
            && !arg.starts_with("--")
            && matches!(
                arg.as_bytes()[1],
                b'X' | b'f' | b'F' | b'H' | b'q' | b't' | b'p'
            )
        {
            (&arg[..2], Some(&arg[2..]))
        } else {
            (arg.as_str(), None)
        };

        let takes = in_list(GH_API_VALUE_OPTS, name);
        let value = match (takes, attached) {
            (_, Some(val)) => val,
            (true, None) => {
                let Some(next) = args.get(i + 1) else {
                    return true;
                };
                i += 1;
                next.as_str()
            }
            (false, None) => "",
        };

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
            _ if !takes && !arg.starts_with('-') && endpoint.is_empty() => endpoint = arg.as_str(),
            _ => {}
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
        return fields.iter().any(|field| {
            let value = field.split_once('=').map_or("", |(_, v)| v);
            field.starts_with('@')
                || value.starts_with('@')
                || field.contains(DYNAMIC)
                || value.to_ascii_lowercase().contains("mutation")
        });
    }
    if method.is_empty() {
        return parameters;
    }
    method != "GET" && method != "HEAD"
}
