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
use codex_change_history::ChangeSourceKind;
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
/// Maximum files walked when expanding directory targets (e.g. `git restore .`, `gofmt -w dir`).
const MAX_DIR_TARGET_FILES: usize = 1024;
/// Maximum child module files followed from a `rustfmt` root file.
const MAX_RUSTFMT_CHILD_MODULES: usize = 256;
/// Maximum file size scanned for `mod <name>;` child module declarations.
const MAX_RUSTFMT_MODULE_SCAN_BYTES: u64 = 1024 * 1024;

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
    baselines: BTreeMap<PathBuf, (FileSnapshot, CoverageStatus, TargetProvenance)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetProvenance {
    Authored,
    Vcs,
    Mixed,
}

impl TargetProvenance {
    fn merge(self, other: Self) -> Self {
        if self == other { self } else { Self::Mixed }
    }
}

#[derive(Default)]
struct ExtractedWriteTargets {
    paths: BTreeMap<PathBuf, TargetProvenance>,
}

impl ExtractedWriteTargets {
    fn insert(&mut self, path: PathBuf, provenance: TargetProvenance) {
        self.paths
            .entry(path)
            .and_modify(|existing| *existing = existing.merge(provenance))
            .or_insert(provenance);
    }
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
        for (path, provenance) in extracted.paths {
            if !can_read(&path) {
                continue;
            }
            let (snapshot, coverage) = read_bounded_snapshot(&path);
            baselines.insert(path, (snapshot, coverage, provenance));
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

    /// True when all statically recognized write targets came from diagnostic
    /// VCS restore operations (e.g. `git restore`, `git checkout --`).
    pub fn is_vcs_only(&self) -> bool {
        !self.baselines.is_empty()
            && self
                .baselines
                .values()
                .all(|(_, _, prov)| *prov == TargetProvenance::Vcs)
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
        let mut all_paths: BTreeMap<PathBuf, (FileSnapshot, CoverageStatus, TargetProvenance)> =
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
                        .and_modify(|(_, _, prov)| {
                            *prov = prov.merge(TargetProvenance::Authored);
                        })
                        .or_insert((
                            FileSnapshot::Missing,
                            CoverageStatus::Complete,
                            TargetProvenance::Authored,
                        ));
                }
            } else {
                all_paths
                    .entry(canonical)
                    .and_modify(|(_, _, prov)| {
                        *prov = prov.merge(TargetProvenance::Authored);
                    })
                    .or_insert((
                        FileSnapshot::Missing,
                        CoverageStatus::Complete,
                        TargetProvenance::Authored,
                    ));
            }
        }

        let mut authored_files = Vec::new();
        let mut vcs_files = Vec::new();
        for (abs_path, (before, before_cov, provenance)) in all_paths {
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
            let change = PendingFileChange {
                rel_path,
                abs_path,
                moved_from: None,
                before,
                after,
                coverage,
            };
            match provenance {
                TargetProvenance::Vcs => vcs_files.push(change),
                TargetProvenance::Authored | TargetProvenance::Mixed => authored_files.push(change),
            }
        }

        if authored_files.is_empty() && vcs_files.is_empty() {
            return Ok(None);
        }

        let store = ChangeHistoryStore::open(store_dir)?;
        let can_reclassify_vcs = matches!(
            metadata.source_kind,
            ChangeSourceKind::ShellWrite
                | ChangeSourceKind::UserShell
                | ChangeSourceKind::VcsDiagnostic
        );

        if authored_files.is_empty() {
            let mut vcs_meta = metadata;
            if can_reclassify_vcs {
                vcs_meta.source_kind = ChangeSourceKind::VcsDiagnostic;
            }
            let record = store.record_change(vcs_meta, vcs_files)?;
            return Ok(Some(record));
        }

        if !vcs_files.is_empty() && can_reclassify_vcs {
            let mut vcs_meta = metadata.clone();
            vcs_meta.source_kind = ChangeSourceKind::VcsDiagnostic;
            if let Some(claim) = &vcs_meta.claim_key
                && !claim.is_empty()
            {
                vcs_meta.claim_key = Some(format!("{claim}:vcs"));
            } else if !vcs_meta.call_id.is_empty() {
                vcs_meta.claim_key = Some(format!(
                    "call:{}:{}:vcs",
                    vcs_meta.thread_id, vcs_meta.call_id
                ));
            }
            let _ = store.record_change(vcs_meta, vcs_files)?;
        } else {
            authored_files.extend(vcs_files);
        }

        let record = store.record_change(metadata, authored_files)?;
        Ok(Some(record))
    }
}

/// Extract statically recognized shell write target paths from script using
/// codex-shell-guard::parse_bash and static_word_value.
pub fn extract_static_write_paths(script: &str, cwd: &Path) -> Vec<PathBuf> {
    extract_write_targets(script, cwd, None)
        .paths
        .into_keys()
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
                        targets.insert(path, TargetProvenance::Authored);
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
                out.insert(
                    resolve_canonical_path(&resolve_path(cwd, Path::new(&raw))),
                    TargetProvenance::Authored,
                );
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
                    out.insert(
                        resolve_canonical_path(&resolve_path(cwd, Path::new(arg))),
                        TargetProvenance::Authored,
                    );
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
                    out.insert(
                        resolve_canonical_path(&resolve_path(cwd, Path::new(arg))),
                        TargetProvenance::Authored,
                    );
                }
            }
        }
        "cp" | "install" => {
            let pos = positional_args(args);
            if let Some(dest) = pos.last()
                && is_capturable_path(dest)
            {
                out.insert(
                    resolve_canonical_path(&resolve_path(cwd, Path::new(dest))),
                    TargetProvenance::Authored,
                );
            }
        }
        "mv" => {
            for arg in positional_args(args) {
                if is_capturable_path(arg) {
                    out.insert(
                        resolve_canonical_path(&resolve_path(cwd, Path::new(arg))),
                        TargetProvenance::Authored,
                    );
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
                        out.insert(
                            resolve_canonical_path(&resolve_path(cwd, Path::new(arg))),
                            TargetProvenance::Authored,
                        );
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
            let resolved = resolve_canonical_path(&resolve_path(cwd, Path::new(arg)));
            if resolved.is_dir() {
                let mut dir_files = Vec::new();
                collect_dir_files_filtered(&resolved, &mut dir_files, MAX_DIR_TARGET_FILES, |p| {
                    p.extension().is_some_and(|ext| ext == "go")
                });
                for file in dir_files {
                    out.insert(file, TargetProvenance::Authored);
                }
            } else {
                out.insert(resolved, TargetProvenance::Authored);
            }
        }
    }
}

fn extract_rustfmt_write_targets(args: &[String], cwd: &Path, out: &mut ExtractedWriteTargets) {
    let mut skip_next = false;
    let mut expect_emit_value = false;
    let mut after_dashdash = false;
    let mut read_only = false;
    let mut skip_children = false;
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
            if arg == "--skip-children" {
                skip_children = true;
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
    let mut visited_modules = BTreeSet::new();
    for arg in paths {
        if is_capturable_path(arg) {
            let resolved = resolve_canonical_path(&resolve_path(cwd, Path::new(arg)));
            out.insert(resolved.clone(), TargetProvenance::Authored);
            if !skip_children {
                collect_rustfmt_child_modules(
                    &resolved,
                    out,
                    &mut visited_modules,
                    MAX_RUSTFMT_CHILD_MODULES,
                );
            }
        }
    }
}

fn collect_rustfmt_child_modules(
    file_path: &Path,
    out: &mut ExtractedWriteTargets,
    visited: &mut BTreeSet<PathBuf>,
    limit: usize,
) {
    if visited.len() >= limit || !visited.insert(file_path.to_path_buf()) {
        return;
    }
    let Ok(meta) = fs::metadata(file_path) else {
        return;
    };
    if !meta.is_file() || meta.len() > MAX_RUSTFMT_MODULE_SCAN_BYTES {
        return;
    }
    let Ok(text) = fs::read_to_string(file_path) else {
        return;
    };
    let Some(parent_dir) = file_path.parent() else {
        return;
    };
    let file_stem = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let is_dir_root = matches!(file_stem, "lib" | "main" | "mod");

    for line in text.lines() {
        if visited.len() >= limit {
            break;
        }
        let Some(mod_name) = parse_out_of_line_mod_decl(line) else {
            continue;
        };
        let mut candidates = Vec::with_capacity(4);
        if is_dir_root {
            candidates.push(parent_dir.join(format!("{mod_name}.rs")));
            candidates.push(parent_dir.join(mod_name).join("mod.rs"));
        } else {
            candidates.push(parent_dir.join(file_stem).join(format!("{mod_name}.rs")));
            candidates.push(parent_dir.join(file_stem).join(mod_name).join("mod.rs"));
            candidates.push(parent_dir.join(format!("{mod_name}.rs")));
            candidates.push(parent_dir.join(mod_name).join("mod.rs"));
        }
        for candidate in candidates {
            if candidate.is_file() {
                let canonical = resolve_canonical_path(&candidate);
                out.insert(canonical.clone(), TargetProvenance::Authored);
                collect_rustfmt_child_modules(&canonical, out, visited, limit);
                break;
            }
        }
    }
}

fn parse_out_of_line_mod_decl(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.starts_with("//") {
        return None;
    }
    let rest = if let Some(after_pub) = trimmed.strip_prefix("pub") {
        let after_pub = after_pub.trim_start();
        if let Some(after_paren) = after_pub.strip_prefix('(') {
            let close = after_paren.find(')')?;
            after_paren[close + 1..].trim_start()
        } else if trimmed
            .as_bytes()
            .get(3)
            .is_some_and(u8::is_ascii_whitespace)
        {
            after_pub
        } else {
            return None;
        }
    } else {
        trimmed
    };
    let after_mod = rest.strip_prefix("mod")?;
    if !after_mod
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    let name_and_semi = after_mod.trim_start();
    let semi_idx = name_and_semi.find(';')?;
    let name = name_and_semi[..semi_idx].trim();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    Some(name)
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
                    insert_git_restore_target(&git_cwd, arg, out);
                }
            }
        }
        "checkout" => {
            let Some(dashdash_pos) = sub_args.iter().position(|arg| arg == "--") else {
                return;
            };
            for arg in &sub_args[dashdash_pos + 1..] {
                if is_capturable_path(arg) {
                    insert_git_restore_target(&git_cwd, arg, out);
                }
            }
        }
        _ => {}
    }
}

fn insert_git_restore_target(git_cwd: &Path, arg: &str, out: &mut ExtractedWriteTargets) {
    let resolved = resolve_canonical_path(&resolve_path(git_cwd, Path::new(arg)));
    if resolved.is_dir() {
        let mut dir_files = Vec::new();
        collect_dir_files_filtered(&resolved, &mut dir_files, MAX_DIR_TARGET_FILES, |_| true);
        for file in dir_files {
            out.insert(file, TargetProvenance::Vcs);
        }
    } else {
        out.insert(resolved, TargetProvenance::Vcs);
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
    collect_dir_files_filtered(dir, out, limit, |_| true);
}

fn collect_dir_files_filtered(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    limit: usize,
    include_file: impl Fn(&Path) -> bool + Copy,
) {
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
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_dir() {
            collect_dir_files_filtered(&path, out, limit, include_file);
        } else if meta.file_type().is_file() && include_file(&path) {
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
