//! Descriptions for Mekugi commands reachable through the native shell tool.

use std::ffi::OsStr;
use std::fmt::Write;
use std::path::Path;

const HELPERS: &[(&str, &str)] = &[
    (
        "inspect_file",
        "Outline source structure as START-END KIND NAME. Use `inspect_file \
         [--json] [--max-tokens N] PATH ...` before reading specific ranges. \
         Output and errors are per file; default budget: 4000 tokens.",
    ),
    (
        "mcat",
        "Read UTF-8 files or inclusive line ranges. Use `mcat [--number] \
         [--max-tokens N] PATH [START:END ...] ...`, e.g. `mcat src/main.rs \
         100:150`. Batch up to 16 reads. `-n N` limits rows within each range; \
         `--tail` selects the last rows and requires a limit. Default budget: \
         6000 tokens. Omitted rows can produce `mread` handles.",
    ),
    (
        "msymbol",
        "Resolve definitions and references with `msymbol [--workspace ROOT] \
         (def|refs) PATH [LINE] SYMBOL [N] ...`. Supply LINE for locals or \
         ambiguous names; N chooses a token occurrence. Batch independent \
         lookups. Rust analysis is offline and can report incomplete coverage. \
         Default budget: 4000 tokens.",
    ),
    (
        "mrun",
        "Keep a bounded window from one foreground command: `mrun (-n N|\
         --max-tokens N) [--head|--tail] [--] COMMAND [ARG...]`. The default \
         is the tail. Data outside the window is discarded, not recoverable. \
         Use native waits for background or interactive commands.",
    ),
    (
        "mread",
        "Continue retained output with `mread REF ... [--stdout|--stderr] \
         [--max-tokens N]`. REF is a returned handle, not a path. Handles \
         share one budget and can fail independently. Ordinary native command \
         truncation has no recovery handle. Default budget: 8000 tokens.",
    ),
    (
        "mchanges",
        "Review captured edits, not Git or current workspace state. Use `mchanges \
         --list` to find your IDs or `mchanges ID[..ID] ... [PATH ...]` for a \
         composed diff. Handoffs need explicit IDs or same-agent ranges and scope. \
         `--summary` shows file statuses; `--history` shows individual attempts. \
         `mchanges revert|apply ID ...` preserves unrelated edits and reports \
         conflicts. Use `--max-tokens` for bounded review, not patch export; \
         validate a full export before feeding it to a mutating consumer.",
    ),
    (
        "mreload",
        "Capture source references with `mreload SELECTION_JSON`. It returns a \
         retained snapshot reference after acquisition. The selection has a \
         `sources` array, e.g. `@path#L2-L4,L8`, `$skill`, `mchanges:ID..ID`. \
         Quote paths containing spaces inside the reference.",
    ),
];

pub(crate) const CHILD_REPLY_SUMMARY: &str = "For substantial messages and final replies to your parent, start the same reply \
     with `## Summary`: give the outcome or blocker and material uncertainty in one \
     to three sentences, then use headings for supporting detail. Keep short replies \
     plain and honor higher-priority exact-output requirements.";

pub(crate) fn shell_guidance(path: &OsStr) -> String {
    let directories: Vec<_> = std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .collect();
    let mut helpers = HELPERS
        .iter()
        .filter(|(name, _)| {
            directories
                .iter()
                .any(|directory| executable(&directory.join(name)))
        })
        .peekable();
    if helpers.peek().is_none() {
        return String::new();
    }

    let mut description = String::from("\n\n### Session shell helpers\n\n");
    description.push_str(include_str!("shell_helpers.md").trim_end());
    if cfg!(target_os = "macos") {
        description.push_str(
            "\n- On macOS, set `shell` to `bash` unless the task requires another shell.",
        );
    }
    for (name, help) in helpers {
        write!(description, "\n\n#### `{name}`\n\n{help}")
            .expect("writing to a String cannot fail");
    }
    description
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

#[cfg(all(test, unix))]
#[path = "guidance_tests.rs"]
mod tests;
