//! Single-shell boundary and mktemp instrumentation.

use std::path::PathBuf;

use codex_shell_guard::command_word_nodes;
use codex_shell_guard::parse_bash;
use codex_shell_guard::protocol::shell_single_quote;
use codex_shell_guard::static_word_value;
use tree_sitter::Node;

use crate::segment::SegmentSpan;
use crate::segment::ShellDialect;

/// Output collection mode for an instrumented shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackingMode {
    /// Non-TTY pipes: segment boundaries synchronize output collection via
    /// transient ctl/ack redirections without occupying caller FDs.
    Pipe,
    /// TTY/PTY mode: keeps the PTY intact and reports segment status/timing
    /// and mktemp creations only.
    Tty,
}

/// Parameters for instrumenting one shell command.
#[derive(Clone, Debug)]
pub struct RewriteOptions {
    pub dialect: ShellDialect,
    pub mode: TrackingMode,
    pub ctl_path: PathBuf,
    pub ack_path: PathBuf,
    pub mktemp_path: PathBuf,
}

/// Wrap each selected segment in __mekugi_b / __mekugi_e boundary hooks.
pub fn rewrite_segment_boundaries(script: &str, segments: &[SegmentSpan]) -> String {
    if segments.len() <= 1 {
        return script.to_string();
    }
    let mut out = String::with_capacity(script.len() + segments.len() * 48);
    let mut offset = 0usize;
    for seg in segments {
        out.push_str(&script[offset..seg.start]);
        if seg.has_heredoc {
            let body = script[seg.start..seg.end].trim_end_matches(['\r', '\n']);
            out.push_str(&format!(
                "{{ __mekugi_b {} && :; {}\n__mekugi_e {} && :; }}",
                seg.index, body, seg.index
            ));
        } else {
            out.push_str(&format!(
                "{{ __mekugi_b {} && :; {}; __mekugi_e {} && :; }}",
                seg.index,
                &script[seg.start..seg.end],
                seg.index
            ));
        }
        offset = seg.end;
    }
    out.push_str(&script[offset..]);
    out
}

/// Produce the complete single-shell instrumented script for script.
pub fn instrument_script(
    script: &str,
    segments: &[SegmentSpan],
    options: &RewriteOptions,
) -> String {
    let ctl = shell_single_quote(&options.ctl_path.to_string_lossy());
    let ack = shell_single_quote(&options.ack_path.to_string_lossy());
    let mk = shell_single_quote(&options.mktemp_path.to_string_lossy());

    let body = if segments.len() > 1 {
        let with_bounds = rewrite_segment_boundaries(script, segments);
        rewrite_explicit_mktemp(&with_bounds)
    } else {
        rewrite_explicit_mktemp(script)
    };

    let single_begin = if segments.len() == 1 {
        "__mekugi_b 0; "
    } else {
        ""
    };

    let preamble = format!(
        concat!(
            "__mekugi_ctl={ctl}; __mekugi_ack={ack}; __mekugi_mk={mk}; __mekugi_cur=0; ",
            "__mekugi_nl='\n'; ",
            "__mekugi_b() {{ __mekugi_s=$?; __mekugi_cur=$1; {{ printf 'b %s\n' \"$1\" > \"$__mekugi_ctl\" && IFS= read -r __mekugi_r < \"$__mekugi_ack\"; }} 2>/dev/null || :; return \"$__mekugi_s\"; }}; ",
            "__mekugi_e() {{ __mekugi_s=$?; {{ printf 'e %s %s\n' \"$1\" \"$__mekugi_s\" > \"$__mekugi_ctl\" && IFS= read -r __mekugi_r < \"$__mekugi_ack\"; }} 2>/dev/null || :; return \"$__mekugi_s\"; }}; ",
            "__mekugi_f() {{ __mekugi_s=$?; {{ printf 'd %s\n' \"$__mekugi_s\" > \"$__mekugi_ctl\" && IFS= read -r __mekugi_r < \"$__mekugi_ack\"; }} 2>/dev/null || :; }}; ",
            "__mekugi_mktemp() {{ ",
            "__mekugi_dry=0; __mekugi_cmd=$1; shift; ",
            "for __mekugi_a in \"$@\"; do case \"$__mekugi_a\" in --) break ;; --dry-run|-u|-*u*) __mekugi_dry=1 ;; esac; done; ",
            "if __mekugi_mr=$(command \"$__mekugi_cmd\" \"$@\"; __mekugi_ms=$?; printf .; exit \"$__mekugi_ms\"); then __mekugi_ms=0; else __mekugi_ms=$?; fi; ",
            "__mekugi_mr=${{__mekugi_mr%.}}; ",
            "if [ \"$__mekugi_ms\" -eq 0 ] && [ \"$__mekugi_dry\" -eq 0 ] && [ -n \"$__mekugi_mr\" ]; then ",
            "__mekugi_mp=${{__mekugi_mr%\"$__mekugi_nl\"}}; ",
            "if [ -e \"$__mekugi_mp\" ] || [ -L \"$__mekugi_mp\" ]; then ",
            "__mekugi_mt=f; [ -d \"$__mekugi_mp\" ] && __mekugi_mt=d; ",
            "{{ printf 'm %s %s %s\\0' \"${{__mekugi_cur-0}}\" \"$__mekugi_mt\" \"$__mekugi_mp\" >> \"$__mekugi_mk\"; }} 2>/dev/null || :; ",
            "fi; fi; ",
            "printf '%s' \"$__mekugi_mr\"; return \"$__mekugi_ms\"; }}; ",
            "if [ \"$(command -v mktemp 2>/dev/null)\" != mktemp ]; then mktemp() {{ __mekugi_mktemp mktemp \"$@\"; }}; fi; ",
            "{{ printf 'o\n' > \"$__mekugi_ctl\" && IFS= read -r __mekugi_r < \"$__mekugi_ack\"; }} 2>/dev/null || :; ",
            "trap __mekugi_f EXIT; ",
            "{single_begin}"
        ),
        ctl = ctl,
        ack = ack,
        mk = mk,
        single_begin = single_begin,
    );

    format!("{preamble}{body}")
}

fn rewrite_explicit_mktemp(script: &str) -> String {
    let Some(tree) = parse_bash(script) else {
        return script.to_string();
    };
    let root = tree.root_node();
    if root.has_error() {
        return script.to_string();
    }

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    collect_explicit_mktemp_edits(root, script, &mut edits);
    if edits.is_empty() {
        return script.to_string();
    }
    edits.sort_by_key(|(start, _, _)| *start);

    let mut out = String::with_capacity(script.len() + edits.len() * 20);
    let mut offset = 0usize;
    for (start, end, replacement) in edits {
        if start < offset {
            continue;
        }
        out.push_str(&script[offset..start]);
        out.push_str(&replacement);
        offset = end;
    }
    out.push_str(&script[offset..]);
    out
}

fn collect_explicit_mktemp_edits(
    node: Node<'_>,
    script: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    if node.kind() == "command" {
        let words = command_word_nodes(node);
        if let Some(first) = words.first()
            && let Some(first_val) = static_word_value(*first, script)
        {
            if is_explicit_mktemp_path(&first_val) {
                edits.push((
                    first.start_byte(),
                    first.start_byte(),
                    "__mekugi_mktemp ".to_string(),
                ));
                return;
            }
            if first_val == "command"
                && let Some(target) = first_command_target(&words[1..], script)
                && (target.val == "mktemp" || is_explicit_mktemp_path(&target.val))
            {
                let target_raw = &script[target.node.start_byte()..target.node.end_byte()];
                edits.push((
                    first.start_byte(),
                    target.node.end_byte(),
                    format!("__mekugi_mktemp {target_raw}"),
                ));
                return;
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_explicit_mktemp_edits(child, script, edits);
    }
}

struct CommandTarget<'a> {
    node: Node<'a>,
    val: String,
}

fn first_command_target<'a>(args: &[Node<'a>], script: &str) -> Option<CommandTarget<'a>> {
    for arg in args {
        let val = static_word_value(*arg, script)?;
        if val == "--" {
            continue;
        }
        if val.starts_with('-') {
            if val.contains('v') || val.contains('V') {
                return None;
            }
            continue;
        }
        return Some(CommandTarget { node: *arg, val });
    }
    None
}

fn is_explicit_mktemp_path(word: &str) -> bool {
    word.contains('/') && word.rsplit('/').next() == Some("mktemp")
}
