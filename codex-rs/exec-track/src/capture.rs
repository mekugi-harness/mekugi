//! Pre/post shell-write and mktemp file-tree capture into codex-change-history.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use codex_change_history::ChangeHistoryError;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeRecord;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use codex_shell_guard::command_word_nodes;
use codex_shell_guard::parse_bash;
use codex_shell_guard::static_word_value;
use tree_sitter::Node;

#[cfg(unix)]
use crate::session::MktempCreation;

/// Maximum inline file bytes read per file snapshot (8 MiB, matching change-evidence.md).
const MAX_CAPTURE_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum files walked inside one mktemp -d directory tree.
const MAX_MKTEMP_TREE_FILES: usize = 1024;

/// Compute the shared deduplication claim_key for (thread_id, call_id) so
/// overlapping apply_patch and ShellWrite observers claim the same logical
/// call at most once.
pub fn shell_write_claim_key(thread_id: &str, call_id: &str) -> String {
    format!("{thread_id}:{call_id}")
}

/// Pre-execution baseline capture for statically recognized shell write targets.
#[derive(Clone, Debug)]
pub struct ShellWriteCapture {
    cwd: PathBuf,
    baselines: BTreeMap<PathBuf, (FileSnapshot, CoverageStatus)>,
}

impl ShellWriteCapture {
    /// Extract statically recognized shell write targets from script and
    /// capture their pre-execution baseline bytes before the shell runs.
    pub fn prepare(script: &str, cwd: &Path) -> Self {
        let paths = extract_static_write_paths(script, cwd);
        let mut baselines = BTreeMap::new();
        for path in paths {
            let snapshot = read_bounded_snapshot(&path);
            baselines.insert(path, snapshot);
        }
        Self {
            cwd: cwd.to_path_buf(),
            baselines,
        }
    }

    /// Number of statically recognized pre-execution candidate paths.
    pub fn candidate_count(&self) -> usize {
        self.baselines.len()
    }

    /// Observe post-execution bytes for all statically recognized paths plus
    /// any mktemp-created files or directory trees, and persist durable
    /// evidence to ChangeHistoryStore (including writes made before a nonzero
    /// exit).
    #[cfg(unix)]
    pub fn finalize_and_persist(
        &self,
        store_dir: &Path,
        metadata: ChangeMetadata,
        mktemp_creations: &[MktempCreation],
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        let workspace_root = metadata.workspace_root.clone();
        let allowed_roots = allowed_mktemp_roots(&workspace_root, &self.cwd);
        let mut all_paths: BTreeMap<PathBuf, (FileSnapshot, CoverageStatus)> =
            self.baselines.clone();

        for created in mktemp_creations {
            let abs = resolve_path(&self.cwd, &created.path);
            let Some(canonical) = admit_mktemp_path(&abs, &allowed_roots) else {
                continue;
            };
            if created.is_dir {
                let mut tree_files = Vec::new();
                collect_dir_files(&canonical, &mut tree_files, MAX_MKTEMP_TREE_FILES);
                for file_path in tree_files {
                    let Some(canonical_file) = admit_mktemp_path(&file_path, &allowed_roots) else {
                        continue;
                    };
                    all_paths
                        .entry(canonical_file)
                        .or_insert((FileSnapshot::Missing, CoverageStatus::Complete));
                }
            } else {
                all_paths
                    .entry(canonical)
                    .or_insert((FileSnapshot::Missing, CoverageStatus::Complete));
            }
        }

        let mut pending_files = Vec::new();
        for (abs_path, (before, before_cov)) in all_paths {
            let (after, after_cov) = read_bounded_snapshot(&abs_path);
            let coverage = before_cov.merge(after_cov);
            if before == after && coverage == CoverageStatus::Complete {
                continue;
            }
            let rel_path = display_rel_path(&workspace_root, &abs_path);
            pending_files.push(PendingFileChange {
                rel_path,
                abs_path,
                moved_from: None,
                before,
                after,
                coverage,
            });
        }

        if pending_files.is_empty() {
            return Ok(None);
        }

        let store = ChangeHistoryStore::open(store_dir)?;
        let record = store.record_change(metadata, pending_files)?;
        Ok(Some(record))
    }
}

/// Extract statically recognized shell write target paths from script using
/// codex-shell-guard::parse_bash and static_word_value.
pub fn extract_static_write_paths(script: &str, cwd: &Path) -> Vec<PathBuf> {
    let Some(tree) = parse_bash(script) else {
        return Vec::new();
    };
    let mut paths = BTreeSet::new();
    let mut current_cwd = resolve_canonical_path(cwd);
    collect_write_paths(tree.root_node(), script, &mut current_cwd, &mut paths);
    paths.into_iter().collect()
}

fn collect_write_paths(
    node: Node<'_>,
    script: &str,
    cwd: &mut PathBuf,
    out: &mut BTreeSet<PathBuf>,
) {
    match node.kind() {
        "subshell" => {
            let mut sub_cwd = cwd.clone();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_write_paths(child, script, &mut sub_cwd, out);
            }
            return;
        }
        "file_redirect" => {
            if is_output_redirect(node, script)
                && let Some(dest) = node.child_by_field_name("destination")
                && let Some(raw) = static_word_value(dest, script)
                && is_capturable_path(&raw)
            {
                out.insert(resolve_canonical_path(&resolve_path(cwd, Path::new(&raw))));
            }
        }
        "command" => {
            let words = command_word_nodes(node);
            let mut static_words: Vec<String> = Vec::new();
            for w in &words {
                if let Some(val) = static_word_value(*w, script) {
                    static_words.push(val);
                } else {
                    static_words.push(String::new());
                }
            }
            if let Some(next_dir) = static_cd_target(&static_words) {
                *cwd = resolve_canonical_path(&resolve_path(cwd, Path::new(&next_dir)));
            }
            extract_command_write_targets(&static_words, cwd, out);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_write_paths(child, script, cwd, out);
    }
}

fn static_cd_target(words: &[String]) -> Option<&str> {
    let first = words.first()?;
    if first != "cd" {
        return None;
    }
    let pos = positional_args(&words[1..]);
    let target = *pos.first()?;
    if target.is_empty() || target == "-" {
        return None;
    }
    Some(target)
}

fn is_output_redirect(node: Node<'_>, script: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            let tok = child.utf8_text(script.as_bytes()).unwrap_or("");
            if matches!(tok, ">" | ">>" | ">|" | "&>" | "&>>") {
                return true;
            }
        }
    }
    false
}

fn extract_command_write_targets(words: &[String], cwd: &Path, out: &mut BTreeSet<PathBuf>) {
    let Some(first) = words.first() else {
        return;
    };
    let cmd_name = first.rsplit('/').next().unwrap_or(first.as_str());
    let args = &words[1..];
    match cmd_name {
        "tee" | "rm" | "touch" => {
            for arg in positional_args(args) {
                if is_capturable_path(arg) {
                    out.insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                }
            }
        }
        "truncate" => {
            let mut skip_next = false;
            for arg in args {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if matches!(arg.as_str(), "-s" | "--size" | "-r" | "--reference") {
                    skip_next = true;
                    continue;
                }
                if arg.starts_with('-') || arg.is_empty() {
                    continue;
                }
                if is_capturable_path(arg) {
                    out.insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                }
            }
        }
        "cp" | "install" => {
            let pos = positional_args(args);
            if let Some(dest) = pos.last()
                && is_capturable_path(dest)
            {
                out.insert(resolve_canonical_path(&resolve_path(cwd, Path::new(dest))));
            }
        }
        "mv" => {
            for arg in positional_args(args) {
                if is_capturable_path(arg) {
                    out.insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                }
            }
        }
        "sed" => {
            let has_in_place = args
                .iter()
                .any(|a| a == "-i" || a.starts_with("-i") || a.starts_with("--in-place"));
            if has_in_place {
                let mut skip_next = false;
                let mut saw_script = false;
                for arg in args {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if matches!(arg.as_str(), "-e" | "-f") {
                        skip_next = true;
                        saw_script = true;
                        continue;
                    }
                    if arg == "-i" || arg.starts_with("-i") || arg.starts_with("--in-place") {
                        continue;
                    }
                    if arg.starts_with('-') || arg.is_empty() {
                        continue;
                    }
                    if !saw_script {
                        saw_script = true;
                        continue;
                    }
                    if is_capturable_path(arg) {
                        out.insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                    }
                }
            }
        }
        _ => {}
    }
}

fn positional_args(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut after_dashdash = false;
    for arg in args {
        if arg.is_empty() {
            continue;
        }
        if !after_dashdash && arg == "--" {
            after_dashdash = true;
            continue;
        }
        if !after_dashdash && arg.starts_with('-') {
            continue;
        }
        out.push(arg.as_str());
    }
    out
}

fn is_capturable_path(raw: &str) -> bool {
    if raw.is_empty() || raw == "-" {
        return false;
    }
    if matches!(
        raw,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
    ) || raw.starts_with("/dev/fd/")
    {
        return false;
    }
    true
}

fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Canonicalize the deepest existing ancestor of `path` and append any
/// remaining non-existent trailing components. This keeps symlinked parent
/// directories (such as `/tmp` -> `/private/tmp` or `link/../config`) and
/// duplicate spellings (`sub/../config`) consistent across capture and undo.
fn resolve_canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let mut suffix = Vec::new();
    let mut cur = path;
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(canonical_parent) = fs::canonicalize(parent) {
            let mut resolved = canonical_parent;
            for part in suffix.into_iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        cur = parent;
    }
    path.to_path_buf()
}

fn allowed_mktemp_roots(workspace_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![workspace_root.to_path_buf(), cwd.to_path_buf()];
    if let Some(session_tmp) = std::env::var_os("SESSION_TMP") {
        candidates.push(PathBuf::from(session_tmp));
    }
    if let Some(tmpdir) = std::env::var_os("TMPDIR") {
        candidates.push(PathBuf::from(tmpdir));
    }
    candidates.push(std::env::temp_dir());
    candidates.push(PathBuf::from("/tmp"));

    let mut roots = Vec::new();
    for candidate in candidates {
        if let Ok(canonical) = fs::canonicalize(&candidate)
            && !roots.contains(&canonical)
        {
            roots.push(canonical);
        }
    }
    roots
}

fn admit_mktemp_path(path: &Path, allowed_roots: &[PathBuf]) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    allowed_roots
        .iter()
        .any(|root| canonical.starts_with(root))
        .then_some(canonical)
}

fn collect_dir_files(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    children.sort();
    for path in children {
        if out.len() >= limit {
            break;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_dir() {
            collect_dir_files(&path, out, limit);
        } else if meta.file_type().is_file() {
            out.push(path);
        }
    }
}

fn read_bounded_snapshot(path: &Path) -> (FileSnapshot, CoverageStatus) {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return (FileSnapshot::Missing, CoverageStatus::Complete);
        }
        Err(err) => {
            return (
                FileSnapshot::Missing,
                CoverageStatus::MissingBaseline {
                    reason: err.to_string(),
                },
            );
        }
    };
    if !meta.is_file() {
        return (FileSnapshot::Missing, CoverageStatus::Complete);
    }
    if meta.len() > MAX_CAPTURE_FILE_BYTES {
        return (
            FileSnapshot::Missing,
            CoverageStatus::Partial {
                reason: format!(
                    "file exceeds {MAX_CAPTURE_FILE_BYTES}-byte capture limit ({} bytes)",
                    meta.len()
                ),
            },
        );
    }
    match fs::read(path) {
        Ok(bytes) => (FileSnapshot::Present(bytes), CoverageStatus::Complete),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            (FileSnapshot::Missing, CoverageStatus::Complete)
        }
        Err(err) => (
            FileSnapshot::Missing,
            CoverageStatus::MissingBaseline {
                reason: err.to_string(),
            },
        ),
    }
}

fn display_rel_path(workspace_root: &Path, abs: &Path) -> String {
    let rel = abs.strip_prefix(workspace_root).unwrap_or_else(|_| {
        if let Ok(canonical_root) = fs::canonicalize(workspace_root) {
            abs.strip_prefix(&canonical_root).unwrap_or(abs)
        } else {
            abs
        }
    });
    rel.to_string_lossy().replace('\\', "/")
}
