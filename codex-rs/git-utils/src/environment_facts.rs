use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use anyhow::Result;
use anyhow::ensure;
use gix::Repository;
use gix::bstr::BStr;
use gix::bstr::BString;
use gix::bstr::ByteSlice;
use gix::hash::ObjectId;
use gix::revision::walk::Sorting;
use gix::status::Item;
use gix::status::index_worktree::iter::Summary;

const MAX_FIELD_BYTES: usize = 512;
const MAX_STATUS_ROWS: usize = 32;
const MAX_REMOTE_ROWS: usize = 16;
const MAX_REMOTE_REFS: usize = 4096;
const MAX_SUBMODULE_DEPTH: usize = 16;
const RECENT_COMMITS: usize = 5;

/// Render local Git facts for the model's environment context without running commands or
/// modifying the repository. The caller runs this on a bounded blocking worker and signals
/// interruption on timeout or cancellation.
pub fn git_environment_facts(cwd: &Path, interrupted: &Arc<AtomicBool>) -> String {
    if interrupted.load(Ordering::Relaxed) {
        return "Git repository: unavailable\n".to_owned();
    }

    let mut permissions = gix::open::Permissions::default();
    // On Windows, obtaining Git's installation-specific system config can launch Git.
    permissions.config.system = !cfg!(windows);
    permissions.attributes.system = !cfg!(windows);
    permissions.config.env = false;
    let options = gix::open::Options::default()
        .permissions(permissions)
        .config_overrides(["core.abbrev=9"]);
    let mut repo = match gix::ThreadSafeRepository::discover_opts(
        cwd,
        Default::default(),
        gix::sec::trust::Mapping {
            full: options.clone(),
            reduced: options,
        },
    ) {
        Ok(repo) => repo.to_thread_local(),
        Err(error)
            if error.classify().any(|cause| {
                cause.error().is::<gix::discover::upwards::Error>()
                    && cause.class() == gix::error::Class::NotFound
            }) =>
        {
            return "Git repository: no\n".to_owned();
        }
        Err(_) => return "Git repository: unavailable\n".to_owned(),
    };
    let Some(root) = repo.workdir() else {
        return "Git repository: no\n".to_owned();
    };

    let mut out = String::from("Git repository: yes\n");
    if root != cwd {
        let _ = writeln!(
            out,
            "Git worktree: {}",
            field(root.to_string_lossy().as_bytes())
        );
    }
    if repo.git_dir() != repo.common_dir() {
        out.push_str("Linked worktree: true\n");
    }
    let head = repo.head().ok();
    let branch = head.as_ref().map_or_else(
        || "unavailable".to_owned(),
        |head| {
            head.referent_name()
                .map_or_else(|| "detached".to_owned(), |name| field(name.shorten()))
        },
    );
    let head_id = head.as_ref().and_then(gix::Head::id);
    let head_display = head_id.as_ref().map_or_else(
        || {
            if head.as_ref().is_some_and(gix::Head::is_unborn) {
                "unborn".to_owned()
            } else {
                "unavailable".to_owned()
            }
        },
        |id| id.shorten_or_id().to_string(),
    );
    let _ = writeln!(out, "Branch: {branch}\nHEAD: {head_display}");
    let head_id = head_id.map(gix::Id::detach);

    let remotes = remote_refs(&repo, interrupted);
    let default_branch = remotes
        .as_ref()
        .ok()
        .and_then(|refs| refs.default_branch.as_deref())
        .unwrap_or("unavailable");
    let _ = writeln!(out, "Default branch: {default_branch}");
    let hooks = repo
        .config_snapshot()
        .string("core.hooksPath")
        .map_or_else(|| "unset (Git default)".to_owned(), |path| field(&path));
    let _ = writeln!(out, "Git hooks path: {hooks}");
    out.push_str("Status:\n");
    match worktree_status(&mut repo, interrupted, 0) {
        Ok(status) => status.render(&mut out),
        Err(_) => out.push_str("unavailable\n"),
    }

    if let Some(head_id) = head_id {
        append_history(&mut out, &repo, head_id, remotes, interrupted);
    }
    out
}

struct RemoteRefs {
    default_branch: Option<String>,
    branches: Vec<(BString, ObjectId)>,
}

fn remote_refs(repo: &Repository, interrupted: &AtomicBool) -> Result<RemoteRefs> {
    let mut default = None;
    let mut ambiguous_default = false;
    let mut branches = Vec::new();
    for (i, reference) in repo.references()?.remote_branches()?.enumerate() {
        ensure!(i < MAX_REMOTE_REFS && !interrupted.load(Ordering::Relaxed));
        let reference = reference?;
        let name: &[u8] = reference.name().as_bstr().as_ref();
        match reference.target() {
            gix::refs::TargetRef::Object(id) => {
                branches.push((reference.name().shorten().to_owned(), id.to_owned()));
            }
            gix::refs::TargetRef::Symbolic(target) => {
                if let Some(remote) = name.strip_suffix(b"/HEAD")
                    && let Some(branch) = target.as_bstr().strip_prefix(remote)
                    && let Some(branch) = branch.strip_prefix(b"/")
                {
                    ambiguous_default |= default
                        .as_deref()
                        .is_some_and(|previous| previous != branch);
                    default = Some(branch.to_vec());
                }
            }
        }
    }
    Ok(RemoteRefs {
        default_branch: default.as_deref().filter(|_| !ambiguous_default).map(field),
        branches,
    })
}

fn append_history(
    out: &mut String,
    repo: &Repository,
    head: ObjectId,
    remotes: Result<RemoteRefs>,
    interrupted: &AtomicBool,
) {
    // Read the first page separately so that a failed remote walk doesn't hide recent commits.
    let recent = recent_commits(repo, head, interrupted);
    let containing = remotes
        .as_ref()
        .ok()
        .and_then(|refs| containing_branches(repo, head, refs, interrupted).ok());
    out.push_str("HEAD in local remote-tracking branches:\n");
    out.push_str(containing.as_deref().unwrap_or("unavailable\n"));
    out.push_str("Recent commits:\n");
    match &recent {
        Ok(recent) => {
            for (_, line) in recent {
                let _ = writeln!(out, "{line}");
            }
        }
        Err(_) => out.push_str("unavailable\n"),
    }
    if let (Ok(recent), Ok(refs)) = (recent, remotes)
        && !refs.branches.is_empty()
    {
        let more = more_unpushed(repo, head, &refs, &recent, interrupted);
        match more {
            Ok(0) => {}
            Ok(count) => {
                let _ = writeln!(out, "(... {count} more unpushed commits)");
            }
            Err(_) => out.push_str("Further unpushed commits: unavailable\n"),
        }
    }
}

fn recent_commits(
    repo: &Repository,
    head: ObjectId,
    interrupted: &AtomicBool,
) -> Result<Vec<(ObjectId, String)>> {
    repo.rev_walk([head])
        .sorting(Sorting::ByCommitTime(Default::default()))
        .selected(|_| !interrupted.load(Ordering::Relaxed))?
        .take(RECENT_COMMITS)
        .map(|info| {
            let info = info?;
            let commit = info.object()?;
            let id = info.id().shorten_or_id();
            let subject = field(&commit.message()?.summary());
            Ok((info.id, format!("{id} {subject}")))
        })
        .collect()
}

fn containing_branches(
    repo: &Repository,
    head: ObjectId,
    refs: &RemoteRefs,
    interrupted: &AtomicBool,
) -> Result<String> {
    let mut out = String::new();
    let mut matches = 0;
    for (name, tip) in &refs.branches {
        ensure!(!interrupted.load(Ordering::Relaxed));
        for info in repo
            .rev_walk([*tip])
            .selected(|_| !interrupted.load(Ordering::Relaxed))?
        {
            if info?.id == head {
                matches += 1;
                if matches > MAX_REMOTE_ROWS {
                    out.push_str("[truncated]\n");
                    return Ok(out);
                }
                let _ = writeln!(out, "{}", field(name));
                break;
            }
        }
    }
    ensure!(!interrupted.load(Ordering::Relaxed));
    if out.is_empty() {
        out.push_str("none\n");
    }
    Ok(out)
}

fn more_unpushed(
    repo: &Repository,
    head: ObjectId,
    refs: &RemoteRefs,
    recent: &[(ObjectId, String)],
    interrupted: &AtomicBool,
) -> Result<usize> {
    ensure!(!interrupted.load(Ordering::Relaxed));
    let mut count = 0;
    for info in repo
        .rev_walk([head])
        .with_hidden(refs.branches.iter().map(|(_, id)| *id))
        .selected(|_| !interrupted.load(Ordering::Relaxed))?
    {
        let id = info?.id;
        if recent.iter().all(|(recent, _)| *recent != id) {
            count += 1;
        }
    }
    ensure!(!interrupted.load(Ordering::Relaxed));
    Ok(count)
}

#[derive(Default)]
struct Status {
    // Sort tracked changes before untracked, with one row for each path.
    rows: BTreeMap<(bool, BString), [char; 2]>,
    truncated: bool,
}

impl Status {
    fn add(&mut self, path: &BStr, change: [Option<char>; 2]) {
        let entry = self
            .rows
            .entry((change == [Some('?'); 2], path.to_owned()))
            .or_insert([' '; 2]);
        for (entry, change) in entry.iter_mut().zip(change) {
            if let Some(change) = change {
                *entry = change;
            }
        }
        if self.rows.len() > MAX_STATUS_ROWS {
            self.rows.pop_last();
            self.truncated = true;
        }
    }

    fn render(self, out: &mut String) {
        if self.rows.is_empty() {
            out.push_str("clean\n");
        }
        for ((_, path), [index, worktree]) in self.rows {
            let _ = writeln!(out, "{index}{worktree} {}", field(&path));
        }
        if self.truncated {
            out.push_str("[status truncated]\n");
        }
    }
}

fn worktree_status(
    repo: &mut Repository,
    interrupted: &Arc<AtomicBool>,
    depth: usize,
) -> Result<Status> {
    use gix::diff::index::Change;
    use gix::status::index_worktree::Item as WorktreeItem;

    ensure!(depth <= MAX_SUBMODULE_DEPTH && !interrupted.load(Ordering::Relaxed));
    let mut inherited_filters = Vec::new();
    if let Some(sections) = repo.config_snapshot().plumbing().sections_by_name("filter") {
        for section in sections {
            if section.meta().source.kind() == gix::config::source::Kind::Repository {
                // In-process status can execute repository-local clean and process drivers.
                ensure!(section.value("clean").is_none() && section.value("process").is_none());
            } else {
                inherited_filters.push(section.id());
            }
        }
    }
    let relative_excludes = repo
        .config_snapshot()
        .trusted_path("core.excludesFile")?
        .filter(|path| path.is_relative())
        .and_then(|path| repo.workdir().map(|root| root.join(path)))
        .map(gix::path::into_bstr)
        .transpose()?;
    if !inherited_filters.is_empty() || relative_excludes.is_some() {
        // The probe does not use global/system filters. Keep other config (e.g. ignore paths).
        let mut config = repo.config_snapshot_mut();
        for id in inherited_filters {
            config.remove_section_by_id(id);
        }
        if let Some(path) = relative_excludes {
            // Git status resolves a relative excludesFile from the worktree root.
            let mut setting = BString::from("core.excludesFile=");
            setting.extend_from_slice(&path);
            config.append_config([setting], gix::config::Source::Api)?;
        }
        config.commit()?;
    }

    let mut status = Status::default();
    for item in repo
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        .dirwalk_options(|options| options.empty_patterns_match_prefix(false))
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .index_worktree_submodules(gix::status::Submodule::Given {
            // Built-in recursion would bypass the filter check in the nested repository.
            ignore: gix::submodule::config::Ignore::Dirty,
            check_dirty: false,
        })
        .should_interrupt_owned(Arc::clone(interrupted))
        .into_iter(None)?
    {
        let item = item?;
        let path = item.location();
        match &item {
            Item::TreeIndex(change) => {
                let code = match change {
                    Change::Addition { .. } => 'A',
                    Change::Deletion { .. } => 'D',
                    Change::Modification {
                        entry_mode,
                        previous_entry_mode,
                        ..
                    } if entry_mode.bits() & 0o170000 != previous_entry_mode.bits() & 0o170000 => {
                        'T'
                    }
                    Change::Modification { .. } => 'M',
                    Change::Rewrite { copy, .. } => {
                        if *copy {
                            'C'
                        } else {
                            'R'
                        }
                    }
                };
                status.add(path, [Some(code), None]);
            }
            Item::IndexWorktree(change) => match change.summary() {
                Some(Summary::Added)
                    if let WorktreeItem::DirectoryContents { entry, .. } = change =>
                {
                    if entry.disk_kind.is_some_and(|kind| kind.is_dir()) {
                        let mut path = path.to_owned();
                        path.push(b'/');
                        status.add(path.as_ref(), [Some('?'); 2]);
                    } else {
                        status.add(path, [Some('?'); 2]);
                    }
                }
                Some(Summary::Conflict) => status.add(path, [Some('U'); 2]),
                Some(summary) => {
                    let code = match summary {
                        Summary::Added | Summary::IntentToAdd => 'A',
                        Summary::Removed => 'D',
                        Summary::TypeChange => 'T',
                        Summary::Modified => 'M',
                        Summary::Renamed => 'R',
                        Summary::Copied => 'C',
                        Summary::Conflict => unreachable!("handled above"),
                    };
                    status.add(path, [None, Some(code)]);
                }
                None => {}
            },
        }
    }
    if let Some(submodules) = repo.submodules()? {
        for submodule in submodules {
            ensure!(!interrupted.load(Ordering::Relaxed));
            if submodule.index_id()?.is_some()
                && submodule.state()?.worktree_checkout
                && let Some(mut nested) = submodule.open()?
                && !worktree_status(&mut nested, interrupted, depth + 1)?
                    .rows
                    .is_empty()
            {
                status.add(submodule.path()?.as_ref(), [None, Some('M')]);
            }
        }
    }
    ensure!(!interrupted.load(Ordering::Relaxed));
    Ok(status)
}

fn field(raw: &[u8]) -> String {
    let mut field = String::new();
    for character in raw[..raw.len().min(MAX_FIELD_BYTES)].chars() {
        if character.is_control() {
            field.extend(character.escape_default());
        } else {
            field.push(character);
        }
    }
    if raw.len() > MAX_FIELD_BYTES {
        field.push_str(" [truncated]");
    }
    field
}

#[cfg(test)]
#[path = "environment_facts_tests.rs"]
mod tests;
