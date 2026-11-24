//! Native `mchanges` CLI backed by `codex-change-history`.

use std::fmt::Write as _;
use std::path::PathBuf;

use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeId;
use codex_change_history::MEKUGI_CHANGE_HISTORY_DIR_ENV_VAR;
use codex_change_history::MutationDirection;
use codex_change_history::compose_diff;
use codex_change_history::compose_summary;
use codex_change_history::expand_id_spec;
use codex_change_history::render_history;

use crate::ToolOutput;

const USAGE: &str = "usage:\n  mchanges --list [ID[..ID] ...]\n  mchanges [--mine | ID[..ID] ...] [--summary | --history] [PATH ...]\n  mchanges revert|apply ID[..ID] ... [PATH ...]\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandMode {
    Diff,
    Summary,
    History,
    List,
    Revert,
    Apply,
}

#[derive(Debug)]
struct ParsedArgs {
    mode: CommandMode,
    mine: bool,
    for_recipient: bool,
    include_history_in_list: bool,
    store_dir: Option<PathBuf>,
    thread_id: Option<String>,
    ids: Vec<ChangeId>,
    paths: Vec<String>,
}

pub(crate) fn execute(args: &[String]) -> ToolOutput {
    let parsed = match parse_args(args) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            return ToolOutput {
                stdout: USAGE.to_string(),
                stderr: String::new(),
                exit_code: 0,
            };
        }
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("mchanges: {err}\n{USAGE}"),
                exit_code: 2,
            };
        }
    };

    let store_dir = match resolve_store_dir(parsed.store_dir.clone()) {
        Ok(dir) => dir,
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("mchanges: {err}\n"),
                exit_code: 1,
            };
        }
    };

    let store = match ChangeHistoryStore::open_existing(&store_dir) {
        Ok(store) => store,
        Err(_)
            if matches!(
                parsed.mode,
                CommandMode::List | CommandMode::Diff | CommandMode::Summary | CommandMode::History
            ) && parsed.ids.is_empty() =>
        {
            return ToolOutput {
                stdout: "No captured changes.\n".to_string(),
                stderr: String::new(),
                exit_code: 0,
            };
        }
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("mchanges: {err}\n"),
                exit_code: 1,
            };
        }
    };

    let thread_id = parsed.thread_id.clone().or_else(|| {
        std::env::var("CODEX_THREAD_ID")
            .ok()
            .filter(|v| !v.is_empty())
    });
    let session_id = std::env::var("CODEX_SESSION_ID").unwrap_or_default();

    match parsed.mode {
        CommandMode::List => run_list(&store, &parsed, thread_id.as_deref()),
        CommandMode::Diff => run_review(&store, &parsed, thread_id.as_deref(), ReviewView::Diff),
        CommandMode::Summary => {
            run_review(&store, &parsed, thread_id.as_deref(), ReviewView::Summary)
        }
        CommandMode::History => {
            run_review(&store, &parsed, thread_id.as_deref(), ReviewView::History)
        }
        CommandMode::Revert => run_mutate(
            &store,
            MutationDirection::Revert,
            &parsed.ids,
            &parsed.paths,
            thread_id.as_deref().unwrap_or("cli"),
            &session_id,
        ),
        CommandMode::Apply => run_mutate(
            &store,
            MutationDirection::Apply,
            &parsed.ids,
            &parsed.paths,
            thread_id.as_deref().unwrap_or("cli"),
            &session_id,
        ),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReviewView {
    Diff,
    Summary,
    History,
}

fn resolve_store_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        return Ok(dir);
    }
    if let Ok(dir) = std::env::var(MEKUGI_CHANGE_HISTORY_DIR_ENV_VAR)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(session_tmp) = std::env::var("SESSION_TMP")
        && !session_tmp.is_empty()
    {
        return Ok(PathBuf::from(session_tmp).join("change-history"));
    }
    Err(format!(
        "{MEKUGI_CHANGE_HISTORY_DIR_ENV_VAR} is not set and no --store directory was provided"
    ))
}

fn parse_args(args: &[String]) -> Result<Option<ParsedArgs>, String> {
    let mut mode = CommandMode::Diff;
    let mut mine = false;
    let mut for_recipient = false;
    let mut include_history_in_list = false;
    let mut store_dir = None;
    let mut thread_id = None;
    let mut ids = Vec::new();
    let mut paths = Vec::new();
    let mut only_paths = false;

    let mut iter = args.iter().peekable();
    if let Some(first) = iter.peek() {
        match first.as_str() {
            "-h" | "--help" => return Ok(None),
            "revert" => {
                mode = CommandMode::Revert;
                iter.next();
            }
            "apply" => {
                mode = CommandMode::Apply;
                iter.next();
            }
            _ => {}
        }
    }

    while let Some(arg) = iter.next() {
        if only_paths {
            paths.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--" => only_paths = true,
            "--list" => {
                if matches!(mode, CommandMode::Revert | CommandMode::Apply) {
                    return Err("--list cannot be combined with revert/apply".to_string());
                }
                mode = CommandMode::List;
            }
            "--summary" => {
                if matches!(
                    mode,
                    CommandMode::Revert | CommandMode::Apply | CommandMode::List
                ) {
                    return Err("--summary cannot be combined with this mode".to_string());
                }
                mode = CommandMode::Summary;
            }
            "--history" => {
                if matches!(mode, CommandMode::Revert | CommandMode::Apply) {
                    return Err("--history cannot be combined with revert/apply".to_string());
                }
                if mode == CommandMode::List {
                    include_history_in_list = true;
                } else {
                    mode = CommandMode::History;
                }
            }
            "--for-recipient" => for_recipient = true,
            "--mine" => {
                if matches!(mode, CommandMode::Revert | CommandMode::Apply) {
                    return Err(
                        "--mine cannot be used with revert/apply; pass explicit change IDs"
                            .to_string(),
                    );
                }
                mine = true;
            }
            "--store" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--store requires a directory path".to_string())?;
                store_dir = Some(PathBuf::from(val));
            }
            "--thread" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--thread requires a thread ID".to_string())?;
                thread_id = Some(val.clone());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`"));
            }
            positional => {
                if paths.is_empty() && looks_like_change_id_or_range(positional) {
                    match expand_id_spec(positional) {
                        Ok(expanded) => ids.extend(expanded),
                        Err(err) => return Err(err.to_string()),
                    }
                } else {
                    paths.push(positional.to_string());
                }
            }
        }
    }

    if matches!(mode, CommandMode::Revert | CommandMode::Apply) && ids.is_empty() {
        return Err("revert and apply require at least one explicit change ID".to_string());
    }

    Ok(Some(ParsedArgs {
        mode,
        mine,
        for_recipient,
        include_history_in_list,
        store_dir,
        thread_id,
        ids,
        paths,
    }))
}

fn looks_like_change_id_or_range(arg: &str) -> bool {
    if arg.contains('/') || arg.contains('\\') {
        return false;
    }
    if arg.contains("..") {
        return true;
    }
    expand_id_spec(arg).is_ok()
}

fn run_list(
    store: &ChangeHistoryStore,
    parsed: &ParsedArgs,
    thread_id: Option<&str>,
) -> ToolOutput {
    let entries = match store.list_entries() {
        Ok(entries) => entries,
        Err(err) => return err_output(err),
    };

    if !parsed.ids.is_empty() {
        for id in &parsed.ids {
            if !entries.iter().any(|e| &e.id == id) {
                return err_output(format!("unknown change ID `{id}`"));
            }
        }
    }

    let mut stdout = String::new();
    for entry in entries {
        if !parsed.include_history_in_list && !entry.source_kind.is_authored() {
            continue;
        }
        if !parsed.ids.is_empty() && !parsed.ids.contains(&entry.id) {
            continue;
        }
        if parsed.mine
            && let Some(tid) = thread_id
            && entry.thread_id != tid
        {
            continue;
        }
        let exit_suffix = match entry.exit_code {
            Some(code) if code != 0 => format!(" exit={code}"),
            _ => String::new(),
        };
        let incomplete = if entry.has_incomplete_coverage {
            " [incomplete]"
        } else {
            ""
        };
        let _ = writeln!(
            &mut stdout,
            "{}\t{}\t{} files{exit_suffix}{incomplete}\t{}",
            entry.id,
            entry.source_kind.label(),
            entry.file_paths.len(),
            entry.file_paths.join(", ")
        );
    }
    if stdout.is_empty() {
        stdout.push_str("No captured changes.\n");
    }
    ToolOutput {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn run_review(
    store: &ChangeHistoryStore,
    parsed: &ParsedArgs,
    thread_id: Option<&str>,
    view: ReviewView,
) -> ToolOutput {
    let ids = if !parsed.ids.is_empty() {
        parsed.ids.clone()
    } else {
        let entries = match store.list_entries() {
            Ok(entries) => entries,
            Err(err) => return err_output(err),
        };
        entries
            .into_iter()
            .filter(|entry| {
                let provenance_ok = view == ReviewView::History || entry.source_kind.is_authored();
                let thread_ok = match thread_id {
                    Some(tid) => entry.thread_id == tid,
                    None => true,
                };
                provenance_ok && thread_ok
            })
            .map(|entry| entry.id)
            .collect()
    };

    if ids.is_empty() {
        return ToolOutput {
            stdout: "No captured changes.\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
    }

    let mut records = match store.load_records(&ids) {
        Ok(records) => records,
        Err(err) => return err_output(err),
    };

    if parsed.for_recipient {
        let Some(recipient) = thread_id else {
            return err_output("recipient identity unavailable");
        };
        records.retain(|record| {
            record.metadata.thread_id != recipient && record.metadata.source_kind.is_authored()
        });
        if records.is_empty() {
            return ToolOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            };
        }
    }
    match view {
        ReviewView::Diff => match compose_diff(store, &records, &parsed.paths) {
            Ok(diffs) => {
                if diffs.is_empty() {
                    return ToolOutput {
                        stdout: "No matching changes.\n".to_string(),
                        stderr: String::new(),
                        exit_code: 0,
                    };
                }
                let mut stdout = String::new();
                for diff in diffs {
                    stdout.push_str(&diff.unified_diff);
                }
                ToolOutput {
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                }
            }
            Err(err) => err_output(err),
        },
        ReviewView::Summary => match compose_summary(store, &records, &parsed.paths) {
            Ok(summary) => {
                if summary.files.is_empty() {
                    return ToolOutput {
                        stdout: "No matching changes.\n".to_string(),
                        stderr: String::new(),
                        exit_code: 0,
                    };
                }
                let mut stdout = String::new();
                for file in &summary.files {
                    if file.is_binary {
                        let _ =
                            writeln!(&mut stdout, "{} {}\t(binary)", file.status, file.rel_path);
                    } else {
                        let _ = writeln!(
                            &mut stdout,
                            "{} {}\t(+{}/-{})",
                            file.status, file.rel_path, file.additions, file.deletions
                        );
                    }
                    for note in &file.coverage_notes {
                        let _ = writeln!(&mut stdout, "  coverage: {note}");
                    }
                }
                let _ = writeln!(
                    &mut stdout,
                    "Total: {} files (+{}/-{})",
                    summary.files.len(),
                    summary.total_additions,
                    summary.total_deletions
                );
                ToolOutput {
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                }
            }
            Err(err) => err_output(err),
        },
        ReviewView::History => {
            let stdout = render_history(&records, &parsed.paths);
            ToolOutput {
                stdout: if stdout.is_empty() {
                    "No matching changes.\n".to_string()
                } else {
                    stdout
                },
                stderr: String::new(),
                exit_code: 0,
            }
        }
    }
}

fn run_mutate(
    store: &ChangeHistoryStore,
    direction: MutationDirection,
    ids: &[ChangeId],
    paths: &[String],
    actor_thread_id: &str,
    actor_session_id: &str,
) -> ToolOutput {
    match store.mutate_workspace(direction, ids, paths, actor_thread_id, actor_session_id) {
        Ok(outcome) => {
            let verb = match direction {
                MutationDirection::Revert => "Reverted",
                MutationDirection::Apply => "Applied",
            };
            let mut stdout = String::new();
            let _ = writeln!(
                &mut stdout,
                "{verb} {} file(s) as {}:\n  {}\nundo: mchanges revert {}",
                outcome.affected_paths.len(),
                outcome.recorded.id,
                outcome.affected_paths.join("\n  "),
                outcome.recorded.id
            );
            ToolOutput {
                stdout,
                stderr: String::new(),
                exit_code: 0,
            }
        }
        Err(err) => err_output(err),
    }
}

fn err_output(err: impl std::fmt::Display) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        stderr: format!("mchanges: {err}\n"),
        exit_code: 1,
    }
}

#[cfg(test)]
#[path = "mchanges_tests.rs"]
mod tests;
