//! Shared pre-write baseline state, path normalization, scope classification,
//! and `ChangeHistoryStore` persistence across Linux and macOS observers.

#[path = "live.rs"]
pub(crate) mod live;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::io::ErrorKind;
use std::io::Read as _;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use codex_change_history::ChangeHistoryError;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeRecord;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use codex_change_history::classify_change_scope;

use crate::active_writers::WriterKey;
use crate::active_writers::finish_call;
use crate::active_writers::register_write;

/// Maximum inline file bytes read per file snapshot (8 MiB, matching change-evidence.md).
const MAX_CAPTURE_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) type ReadPolicyFn = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

struct ObservedFile {
    // None is a non-regular path or an unavailable baseline, not a missing file.
    before: Option<Arc<FileSnapshot>>,
    coverage: CoverageStatus,
    keys: Vec<WriterKey>,
    provenance: WriteProvenance,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteProvenance {
    Authored,
    VcsDiagnostic,
    Mixed,
}

/// Mutable per-command write observation state shared between the OS supervisor
/// thread and final settlement.
pub(crate) struct ObservationState {
    call_id: String,
    ignored_root: PathBuf,
    workspace_root: Option<PathBuf>,
    ignored_cache_roots: Vec<PathBuf>,
    can_read: ReadPolicyFn,
    baselines: BTreeMap<PathBuf, ObservedFile>,
    partial_reasons: Vec<String>,
}

impl ObservationState {
    #[cfg(test)]
    pub(crate) fn new(call_id: String, ignored_root: PathBuf, can_read: ReadPolicyFn) -> Self {
        Self::new_with_workspace(call_id, ignored_root, None, can_read)
    }

    pub(crate) fn new_with_workspace(
        call_id: String,
        ignored_root: PathBuf,
        workspace_root: Option<PathBuf>,
        can_read: ReadPolicyFn,
    ) -> Self {
        Self::new_with_roots(
            call_id,
            ignored_root,
            workspace_root,
            default_ignored_cache_roots(),
            can_read,
        )
    }

    pub(crate) fn new_with_roots(
        call_id: String,
        ignored_root: PathBuf,
        workspace_root: Option<PathBuf>,
        ignored_cache_roots: Vec<PathBuf>,
        can_read: ReadPolicyFn,
    ) -> Self {
        let ignored_root = resolve_canonical_path(&ignored_root);
        let workspace_root = workspace_root.map(|ws| resolve_canonical_path(&ws));
        let mut expanded = Vec::with_capacity(ignored_cache_roots.len() * 2);
        for root in ignored_cache_roots {
            if root.as_os_str().is_empty() {
                continue;
            }
            let canonical = resolve_canonical_path(&root);
            if !expanded.contains(&root) {
                expanded.push(root);
            }
            if !expanded.contains(&canonical) {
                expanded.push(canonical);
            }
        }
        Self {
            call_id,
            ignored_root,
            workspace_root,
            ignored_cache_roots: expanded,
            can_read,
            baselines: BTreeMap::new(),
            partial_reasons: Vec::new(),
        }
    }

    /// Lazily snapshot pre-write bytes the first time this command touches `path`.
    pub(crate) fn record_pre_write(&mut self, raw_path: &Path) {
        self.record_pre_write_with_provenance(raw_path, false);
    }

    /// Lazily snapshot pre-write bytes, tracking whether the writing subprocess
    /// is a VCS-diagnostic git operation (e.g. `git restore`/`git checkout`).
    pub(crate) fn record_pre_write_with_provenance(
        &mut self,
        raw_path: &Path,
        is_vcs_diagnostic: bool,
    ) {
        if is_always_ignored_path(raw_path, &self.ignored_root) {
            return;
        }
        let path = resolve_canonical_path(raw_path);
        if self.should_ignore(&path) {
            return;
        }
        let provenance = if is_vcs_diagnostic {
            WriteProvenance::VcsDiagnostic
        } else {
            WriteProvenance::Authored
        };
        if let Some(existing) = self.baselines.get_mut(&path) {
            if existing.provenance != provenance {
                existing.provenance = WriteProvenance::Mixed;
            }
            return;
        }
        if !(self.can_read)(&path) {
            return;
        }
        let keys = WriterKey::keys_for_path(&path);
        let (snapshot, mut coverage) = read_bounded_snapshot(&path);
        if register_write(&self.call_id, &keys) {
            coverage = coverage.merge(CoverageStatus::Partial {
                reason: "concurrent write to same file".to_string(),
            });
        }
        self.baselines.insert(
            path,
            ObservedFile {
                before: snapshot.map(Arc::new),
                coverage,
                keys,
                provenance,
            },
        );
    }

    /// Record an explicit honesty marker (e.g., macOS SIP-protected binary executed).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn record_partial_reason(&mut self, reason: String) {
        if !self.partial_reasons.contains(&reason) {
            self.partial_reasons.push(reason);
        }
    }

    /// Release any held active-writer registrations without persisting.
    pub(crate) fn release_active_writers(&mut self) {
        let baselines = std::mem::take(&mut self.baselines);
        self.partial_reasons.clear();
        let keys = Self::collect_keys(&baselines);
        if !keys.is_empty() {
            let _ = finish_call(&self.call_id, &keys);
        }
    }

    fn collect_keys(baselines: &BTreeMap<PathBuf, ObservedFile>) -> Vec<WriterKey> {
        baselines
            .values()
            .flat_map(|obs| obs.keys.iter().cloned())
            .collect()
    }

    /// Observe post-execution bytes for all touched paths and persist durable
    /// evidence into `ChangeHistoryStore`.
    pub(crate) fn finalize_and_persist(
        &mut self,
        store_dir: &Path,
        workspace_root: &Path,
        session_tmp: Option<&Path>,
        metadata: ChangeMetadata,
    ) -> Result<Option<ChangeRecord>, ChangeHistoryError> {
        let baselines = std::mem::take(&mut self.baselines);
        let partial_reasons = std::mem::take(&mut self.partial_reasons);
        let keys = Self::collect_keys(&baselines);
        let conflicted = finish_call(&self.call_id, &keys);

        let mut authored_files = Vec::new();
        let mut vcs_files = Vec::new();
        for (abs_path, obs) in baselines {
            if !(self.can_read)(&abs_path)
                || fs::canonicalize(&abs_path)
                    .ok()
                    .is_some_and(|canon| !(self.can_read)(&canon))
            {
                continue;
            }
            let (after, after_cov) = read_bounded_snapshot(&abs_path);
            let before = obs
                .before
                .map_or(FileSnapshot::Missing, Arc::unwrap_or_clone);
            let after = after.unwrap_or(FileSnapshot::Missing);
            let mut coverage = obs.coverage.merge(after_cov);
            if obs.keys.iter().any(|k| conflicted.contains(k)) {
                coverage = coverage.merge(CoverageStatus::Partial {
                    reason: "concurrent write to same file".to_string(),
                });
            }
            if !partial_reasons.is_empty() {
                coverage = coverage.merge(CoverageStatus::Partial {
                    reason: partial_reasons.join("; "),
                });
            }
            if before == after && coverage == CoverageStatus::Complete {
                continue;
            }
            let rel_path = display_rel_path(workspace_root, &abs_path);
            let scope = classify_change_scope(&abs_path, workspace_root, session_tmp);
            let change = PendingFileChange {
                rel_path,
                abs_path,
                moved_from: None,
                before,
                after,
                coverage,
                scope,
            };
            if obs.provenance == WriteProvenance::VcsDiagnostic {
                vcs_files.push(change);
            } else {
                authored_files.push(change);
            }
        }

        if authored_files.is_empty() && vcs_files.is_empty() {
            return Ok(None);
        }

        let store = ChangeHistoryStore::open(store_dir)?;
        let mut vcs_record = None;
        if !vcs_files.is_empty() {
            let mut vcs_meta = metadata.clone();
            vcs_meta.source_kind = ChangeSourceKind::VcsDiagnostic;
            vcs_meta.claim_key = Some(match vcs_meta.claim_key {
                Some(k) => format!("{k}:vcs"),
                None => format!("{}:{}:vcs", vcs_meta.thread_id, vcs_meta.call_id),
            });
            vcs_record = Some(store.record_change(vcs_meta, vcs_files)?);
        }
        if !authored_files.is_empty() {
            let record = store.record_change(metadata, authored_files)?;
            return Ok(Some(record));
        }
        Ok(vcs_record)
    }

    fn should_ignore(&self, path: &Path) -> bool {
        should_ignore_path(
            path,
            &self.ignored_root,
            self.workspace_root.as_deref(),
            &self.ignored_cache_roots,
        )
    }
}

fn should_ignore_path(
    path: &Path,
    ignored_root: &Path,
    workspace_root: Option<&Path>,
    ignored_cache_roots: &[PathBuf],
) -> bool {
    if is_always_ignored_path(path, ignored_root) {
        return true;
    }
    // Workspace root takes precedence over toolchain cache ignores (for example,
    // if a repository is checked out under an otherwise-ignored cache root).
    if let Some(ws) = workspace_root
        && path.starts_with(ws)
    {
        return false;
    }
    let s = path.to_string_lossy();
    if s.starts_with("/run/user/") {
        return true;
    }
    ignored_cache_roots
        .iter()
        .any(|root| path.starts_with(root))
}

fn is_always_ignored_path(path: &Path, ignored_root: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return true;
    }
    if path.starts_with(ignored_root) {
        return true;
    }
    let s = path.to_string_lossy();
    if s == "/dev/null"
        || s == "/dev/tty"
        || s == "/dev/stdout"
        || s == "/dev/stderr"
        || s.starts_with("/dev/")
        || s.starts_with("/proc/")
        || s.starts_with("/sys/")
    {
        return true;
    }
    is_git_internal_path(path)
}

fn is_git_internal_path(path: &Path) -> bool {
    path.components()
        .any(|comp| matches!(comp, Component::Normal(name) if name == ".git"))
}

fn default_ignored_cache_roots() -> Vec<PathBuf> {
    const HOME_IGNORED_CACHE_PATHS: &[&str] = &[
        ".cache/go-build",
        ".config/go/telemetry",
        "go/pkg/mod/cache",
        ".npm/_cacache",
        ".npm/_logs",
        ".cache/pip",
        ".cache/uv",
        ".cargo/registry",
        ".cargo/git",
    ];
    let non_empty_env_os = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());

    let mut roots = Vec::new();
    if let Some(gocache) = non_empty_env_os("GOCACHE").filter(|v| v != "off") {
        roots.push(PathBuf::from(gocache));
    }
    if let Some(gomodcache) = non_empty_env_os("GOMODCACHE") {
        roots.push(PathBuf::from(gomodcache).join("cache"));
    }
    if let Some(gopath) = non_empty_env_os("GOPATH") {
        roots.extend(
            std::env::split_paths(&gopath)
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.join("pkg/mod/cache")),
        );
    }
    if let Some(cargo_home) = non_empty_env_os("CARGO_HOME") {
        let cargo = PathBuf::from(cargo_home);
        roots.push(cargo.join("registry"));
        roots.push(cargo.join("git"));
    }
    if let Some(xdg_runtime) = non_empty_env_os("XDG_RUNTIME_DIR") {
        roots.push(PathBuf::from(xdg_runtime));
    }
    if let Some(home) = non_empty_env_os("HOME") {
        let home = PathBuf::from(home);
        roots.extend(HOME_IGNORED_CACHE_PATHS.iter().map(|rel| home.join(rel)));
    }
    roots
}

pub(crate) fn resolve_canonical_path(path: &Path) -> PathBuf {
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

fn read_bounded_snapshot(path: &Path) -> (Option<FileSnapshot>, CoverageStatus) {
    let opened = open_observed_file(path).and_then(|file| {
        let metadata = file.metadata()?;
        Ok((file, metadata))
    });
    let (file, meta) = match opened {
        Ok(opened) => opened,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return (Some(FileSnapshot::Missing), CoverageStatus::Complete);
        }
        Err(err) => {
            return (
                None,
                CoverageStatus::MissingBaseline {
                    reason: err.to_string(),
                },
            );
        }
    };
    if !meta.file_type().is_file() {
        return (None, CoverageStatus::Complete);
    }
    if meta.len() > MAX_CAPTURE_FILE_BYTES {
        return (
            None,
            CoverageStatus::Partial {
                reason: format!(
                    "file exceeds {MAX_CAPTURE_FILE_BYTES}-byte capture limit ({} bytes)",
                    meta.len()
                ),
            },
        );
    }
    let mut bytes = Vec::new();
    match file
        .take(MAX_CAPTURE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        Ok(len) if len as u64 <= MAX_CAPTURE_FILE_BYTES => {
            (Some(FileSnapshot::Present(bytes)), CoverageStatus::Complete)
        }
        Ok(_) => (
            None,
            CoverageStatus::Partial {
                reason: format!("file exceeds {MAX_CAPTURE_FILE_BYTES}-byte capture limit"),
            },
        ),
        Err(err) => (
            None,
            CoverageStatus::MissingBaseline {
                reason: err.to_string(),
            },
        ),
    }
}

/// Open the canonical observation path without traversing replacement symlinks.
#[cfg(all(unix, not(target_os = "macos")))]
fn open_observed_file(path: &Path) -> io::Result<fs::File> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    use std::os::fd::FromRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    #[cfg(target_os = "linux")]
    const OPEN_DIR: libc::c_int = libc::O_PATH | libc::O_DIRECTORY;
    #[cfg(not(target_os = "linux"))]
    const OPEN_DIR: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;

    let path = path
        .strip_prefix("/")
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "expected an absolute file path"))?;
    let mut components = path.components().peekable();
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_DIR)
        .open("/")?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "non-canonical file path",
            ));
        };
        let name = CString::new(name.as_bytes())?;
        let access = if components.peek().is_some() {
            OPEN_DIR
        } else {
            libc::O_RDONLY | libc::O_NONBLOCK
        };
        // Each call resolves just one name under an already-open directory and creates
        // a new descriptor. The current directory fd remains valid through the call.
        let fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                name.as_ptr(),
                access | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        file = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(file)
}

#[cfg(target_os = "macos")]
fn open_observed_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW_ANY | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_observed_file(_path: &Path) -> io::Result<fs::File> {
    Err(io::Error::from(ErrorKind::Unsupported))
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
