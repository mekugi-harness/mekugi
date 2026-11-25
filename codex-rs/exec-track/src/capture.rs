//! Pre/post shell-write and mktemp file-tree capture into codex-change-history.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Component;
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
use codex_shell_guard::static_word_value_with_expansion;
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
    vcs_only: bool,
}

#[derive(Default)]
struct ExtractedWriteTargets {
    paths: BTreeSet<PathBuf>,
    has_authored: bool,
    has_vcs: bool,
}

impl ShellWriteCapture {
    /// Extract statically recognized shell write targets from script and
    /// capture their pre-execution baseline bytes before the shell runs.
    pub fn prepare(script: &str, cwd: &Path) -> Self {
        Self::prepare_with_policy(script, cwd, None, |_| true)
    }

    /// Extract statically recognized shell write targets from script and
    /// capture pre-execution baselines only for paths permitted by `can_read`.
    pub fn prepare_with_policy(
        script: &str,
        cwd: &Path,
        session_tmp: Option<&Path>,
        can_read: impl Fn(&Path) -> bool,
    ) -> Self {
        let extracted = extract_write_targets(script, cwd, session_tmp);
        let mut baselines = BTreeMap::new();
        for path in extracted.paths {
            if !can_read(&path) {
                continue;
            }
            let snapshot = read_bounded_snapshot(&path);
            baselines.insert(path, snapshot);
        }
        Self {
            cwd: cwd.to_path_buf(),
            baselines,
            vcs_only: extracted.has_vcs && !extracted.has_authored,
        }
    }

    /// Number of statically recognized pre-execution candidate paths.
    pub fn candidate_count(&self) -> usize {
        self.baselines.len()
    }

    /// True when all statically recognized write targets came from diagnostic
    /// VCS restore operations (e.g. `git restore`, `git checkout --`).
    pub fn is_vcs_only(&self) -> bool {
        self.vcs_only
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
        self.finalize_and_persist_with_policy(store_dir, metadata, mktemp_creations, |_| true)
    }

    /// Observe post-execution bytes only for paths permitted by `can_read`,
    /// preventing sandboxed commands from using the writable `mktemp` log to
    /// read files denied by their sandbox policy.
    #[cfg(unix)]
    pub fn finalize_and_persist_with_policy(
        &self,
        store_dir: &Path,
        metadata: ChangeMetadata,
        mktemp_creations: &[MktempCreation],
        can_read: impl Fn(&Path) -> bool,
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        let workspace_root = metadata.workspace_root.clone();
        let allowed_roots = allowed_mktemp_roots(&workspace_root, &self.cwd);
        let mut all_paths: BTreeMap<PathBuf, (FileSnapshot, CoverageStatus)> =
            self.baselines.clone();

        for created in mktemp_creations {
            let abs = resolve_path(&self.cwd, &created.path);
            if !can_read(&abs) {
                continue;
            }
            let Some(canonical) = admit_mktemp_path(&abs, &allowed_roots) else {
                continue;
            };
            if !can_read(&canonical) {
                continue;
            }
            if created.is_dir {
                let mut tree_files = Vec::new();
                collect_dir_files(&canonical, &mut tree_files, MAX_MKTEMP_TREE_FILES);
                for file_path in tree_files {
                    if !can_read(&file_path) {
                        continue;
                    }
                    let Some(canonical_file) = admit_mktemp_path(&file_path, &allowed_roots) else {
                        continue;
                    };
                    if !can_read(&canonical_file) {
                        continue;
                    }
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
            if !can_read(&abs_path)
                || fs::canonicalize(&abs_path)
                    .ok()
                    .is_some_and(|canonical| !can_read(&canonical))
            {
                continue;
            }
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
    extract_write_targets(script, cwd, None)
        .paths
        .into_iter()
        .collect()
}

fn extract_write_targets(
    script: &str,
    cwd: &Path,
    session_tmp: Option<&Path>,
) -> ExtractedWriteTargets {
    let Some(tree) = parse_bash(script) else {
        return ExtractedWriteTargets::default();
    };
    let mut targets = ExtractedWriteTargets::default();
    let mut current_cwd = resolve_canonical_path(cwd);
    collect_write_paths(tree.root_node(), script, &mut current_cwd, &mut targets);
    if let Some(session_tmp) = session_tmp.filter(|path| path.is_absolute())
        && script.contains("SESSION_TMP")
        && !tree.root_node().has_error()
    {
        collect_scratch_redirects(tree.root_node(), script, session_tmp, &mut targets);
    }
    targets
}

fn collect_scratch_redirects(
    root: Node<'_>,
    script: &str,
    session_tmp: &Path,
    targets: &mut ExtractedWriteTargets,
) {
    let Some(binding) = session_tmp.to_str() else {
        return;
    };
    let session_tmp = resolve_canonical_path(session_tmp);
    let resolve = |text: &str, quoted: bool| {
        if matches!(text, "$SESSION_TMP" | "${SESSION_TMP}")
            && (quoted || !binding.contains([' ', '\t', '\n']))
        {
            Some(binding.to_owned())
        } else {
            None
        }
    };
    let mut cursor = root.walk();
    for stmt in root.named_children(&mut cursor) {
        let command = match stmt.kind() {
            "comment" => continue,
            "command" => stmt,
            "redirected_statement" => {
                let Some(command) = stmt.child_by_field_name("body") else {
                    break;
                };
                if command.kind() != "command" {
                    break;
                }
                command
            }
            _ => break,
        };
        if !has_stable_scratch_binding(stmt, script) {
            break;
        }
        for statement in [stmt, command] {
            let mut cursor = statement.walk();
            for redirect in statement.named_children(&mut cursor) {
                if redirect.kind() != "file_redirect" {
                    continue;
                }
                let mut tokens = redirect.walk();
                if !is_output_redirect(redirect, script)
                    && !redirect
                        .children(&mut tokens)
                        .any(|node| node.kind() == "<>")
                {
                    continue;
                }
                if let Some(word) = redirect.child_by_field_name("destination")
                    && static_word_value(word, script).is_none()
                    && let Some(path) = static_word_value_with_expansion(word, script, resolve)
                    && !path.contains(['*', '?', '[', '{', '}'])
                    && Path::new(&path).is_absolute()
                {
                    let path = resolve_canonical_path(Path::new(&path));
                    if path != session_tmp && path.starts_with(&session_tmp) {
                        targets.paths.insert(path);
                        targets.has_authored = true;
                    }
                }
            }
            if stmt == command {
                break;
            }
        }
        // Stop before subsequent commands after anything that may change the
        // parent shell's binding, such as a sourcing builtin or shell function.
        let words = command_word_nodes(command);
        if !words
            .first()
            .and_then(|word| static_word_value(*word, script))
            .is_some_and(|name| {
                matches!(
                    name.as_str(),
                    "cat"
                        | "mkdir"
                        | "tee"
                        | "touch"
                        | "echo"
                        | "printf"
                        | "set"
                        | "cd"
                        | "true"
                        | "false"
                        | ":"
                )
            })
        {
            break;
        }
    }
}

fn has_stable_scratch_binding(node: Node<'_>, script: &str) -> bool {
    match node.kind() {
        "variable_assignment" | "arithmetic_expansion" => return false,
        "expansion" => {
            if node
                .utf8_text(script.as_bytes())
                .is_ok_and(|text| text.contains(":=") || text.contains('='))
            {
                return false;
            }
        }
        "heredoc_body" | "command_substitution" | "process_substitution" => return true,
        _ => {}
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .all(|child| has_stable_scratch_binding(child, script))
}

fn collect_write_paths(
    node: Node<'_>,
    script: &str,
    cwd: &mut PathBuf,
    out: &mut ExtractedWriteTargets,
) {
    match node.kind() {
        "subshell" | "command_substitution" => {
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
                out.paths
                    .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(&raw))));
                out.has_authored = true;
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

fn extract_command_write_targets(words: &[String], cwd: &Path, out: &mut ExtractedWriteTargets) {
    let Some(first) = words.first() else {
        return;
    };
    let cmd_name = first.rsplit('/').next().unwrap_or(first.as_str());
    let args = &words[1..];
    match cmd_name {
        "tee" | "rm" | "touch" => {
            for arg in positional_args(args) {
                if is_capturable_path(arg) {
                    out.paths
                        .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                    out.has_authored = true;
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
                    out.paths
                        .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                    out.has_authored = true;
                }
            }
        }
        "cp" | "install" => {
            let pos = positional_args(args);
            if let Some(dest) = pos.last()
                && is_capturable_path(dest)
            {
                out.paths
                    .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(dest))));
                out.has_authored = true;
            }
        }
        "mv" => {
            for arg in positional_args(args) {
                if is_capturable_path(arg) {
                    out.paths
                        .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                    out.has_authored = true;
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
                        out.paths
                            .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
                        out.has_authored = true;
                    }
                }
            }
        }
        "gofmt" | "goimports" => {
            extract_gofmt_write_targets(args, cwd, out);
        }
        "rustfmt" => {
            extract_rustfmt_write_targets(args, cwd, out);
        }
        "git" => {
            extract_git_restore_targets(args, cwd, out);
        }
        _ => {}
    }
}

fn extract_gofmt_write_targets(args: &[String], cwd: &Path, out: &mut ExtractedWriteTargets) {
    let mut has_write_flag = false;
    let mut skip_next = false;
    let mut after_dashdash = false;
    let mut paths = Vec::new();

    for arg in args {
        if arg.is_empty() {
            continue;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if !after_dashdash && arg == "--" {
            after_dashdash = true;
            continue;
        }
        if !after_dashdash && arg.starts_with('-') {
            if matches!(arg.as_str(), "-r" | "-cpuprofile" | "-local" | "-srcdir") {
                skip_next = true;
                continue;
            }
            if arg == "-w" || arg == "--w" || (!arg.starts_with("--") && arg[1..].contains('w')) {
                has_write_flag = true;
            }
            continue;
        }
        paths.push(arg.as_str());
    }

    if !has_write_flag {
        return;
    }
    for arg in paths {
        if is_capturable_path(arg) {
            out.paths
                .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
            out.has_authored = true;
        }
    }
}

fn extract_rustfmt_write_targets(args: &[String], cwd: &Path, out: &mut ExtractedWriteTargets) {
    let mut skip_next = false;
    let mut expect_emit_value = false;
    let mut after_dashdash = false;
    let mut read_only = false;
    let mut paths = Vec::new();

    for arg in args {
        if arg.is_empty() {
            continue;
        }
        if expect_emit_value {
            expect_emit_value = false;
            if arg != "files" {
                read_only = true;
            }
            continue;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if !after_dashdash && arg == "--" {
            after_dashdash = true;
            continue;
        }
        if !after_dashdash && arg.starts_with('-') {
            if matches!(
                arg.as_str(),
                "--check"
                    | "--print-config"
                    | "--list-files"
                    | "-h"
                    | "--help"
                    | "-V"
                    | "--version"
            ) {
                read_only = true;
                continue;
            }
            if arg == "--emit" {
                expect_emit_value = true;
                continue;
            }
            if let Some(val) = arg.strip_prefix("--emit=") {
                if val != "files" {
                    read_only = true;
                }
                continue;
            }
            if matches!(
                arg.as_str(),
                "--edition"
                    | "--config"
                    | "--config-path"
                    | "--color"
                    | "--file-lines"
                    | "--style-edition"
            ) {
                skip_next = true;
                continue;
            }
            continue;
        }
        paths.push(arg.as_str());
    }

    if read_only {
        return;
    }
    for arg in paths {
        if is_capturable_path(arg) {
            out.paths
                .insert(resolve_canonical_path(&resolve_path(cwd, Path::new(arg))));
            out.has_authored = true;
        }
    }
}

fn extract_git_restore_targets(args: &[String], cwd: &Path, out: &mut ExtractedWriteTargets) {
    let mut git_cwd = cwd.to_path_buf();
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg.is_empty() {
            idx += 1;
            continue;
        }
        if arg == "-C" {
            if let Some(dir) = args.get(idx + 1)
                && !dir.is_empty()
            {
                git_cwd = resolve_canonical_path(&resolve_path(&git_cwd, Path::new(dir)));
            }
            idx += 2;
            continue;
        }
        if let Some(dir) = arg.strip_prefix("-C")
            && !dir.is_empty()
        {
            git_cwd = resolve_canonical_path(&resolve_path(&git_cwd, Path::new(dir)));
            idx += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix" | "--config-env"
        ) {
            idx += 2;
            continue;
        }
        if arg.starts_with('-') {
            idx += 1;
            continue;
        }
        break;
    }

    let Some(subcmd) = args.get(idx).map(String::as_str) else {
        return;
    };
    let sub_args = &args[idx + 1..];
    match subcmd {
        "restore" => {
            let mut staged = false;
            let mut worktree = false;
            let mut skip_next = false;
            let mut after_dashdash = false;
            let mut paths = Vec::new();

            for arg in sub_args {
                if arg.is_empty() {
                    continue;
                }
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if !after_dashdash && arg == "--" {
                    after_dashdash = true;
                    continue;
                }
                if !after_dashdash && arg.starts_with('-') {
                    if matches!(
                        arg.as_str(),
                        "-s" | "--source" | "--conflict" | "--pathspec-from-file"
                    ) {
                        skip_next = true;
                        continue;
                    }
                    if arg == "-S" || arg == "--staged" {
                        staged = true;
                        continue;
                    }
                    if arg == "-W" || arg == "--worktree" {
                        worktree = true;
                        continue;
                    }
                    if !arg.starts_with("--") {
                        if arg[1..].contains('S') {
                            staged = true;
                        }
                        if arg[1..].contains('W') {
                            worktree = true;
                        }
                    }
                    continue;
                }
                paths.push(arg.as_str());
            }

            if staged && !worktree {
                return;
            }
            for arg in paths {
                if is_capturable_path(arg) {
                    out.paths.insert(resolve_canonical_path(&resolve_path(
                        &git_cwd,
                        Path::new(arg),
                    )));
                    out.has_vcs = true;
                }
            }
        }
        "checkout" => {
            let Some(dashdash_pos) = sub_args.iter().position(|arg| arg == "--") else {
                return;
            };
            for arg in &sub_args[dashdash_pos + 1..] {
                if is_capturable_path(arg) {
                    out.paths.insert(resolve_canonical_path(&resolve_path(
                        &git_cwd,
                        Path::new(arg),
                    )));
                    out.has_vcs = true;
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
    let mut resolved = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let candidate = resolved.join("..");
                if let Ok(canonical) = fs::canonicalize(&candidate) {
                    resolved = canonical;
                } else if !resolved.pop() {
                    resolved.push(comp.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                resolved.push(comp.as_os_str());
                if let Ok(canonical) = fs::canonicalize(&resolved) {
                    resolved = canonical;
                }
            }
        }
    }
    resolved
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
