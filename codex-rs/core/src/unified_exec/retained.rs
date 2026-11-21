//! Persistent thread-scoped storage for retained unified-exec command outputs.
//!
//! Layout under `<codex_home>/retained_exec/<thread_id>/`:
//! - `<process_id>.bin`: raw head-bounded output bytes (up to 16 MiB).
//! - `<process_id>.json`: atomically written cursor and completion metadata.
//!
//! Forked threads snapshot their source thread's metadata read-only at fork
//! creation, pointing `source_thread_id` at the owning ancestor thread so
//! large `.bin` files are never duplicated and parent files are never mutated.

use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;
use tracing::warn;
use uuid::Uuid;

pub(crate) const RETAINED_EXEC_SUBDIR: &str = "retained_exec";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RetainedExecMeta {
    pub(crate) process_id: i32,
    pub(crate) call_id: String,
    pub(crate) environment_id: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) completed: bool,
    pub(crate) retained_bytes: usize,
    pub(crate) discarded_bytes: u64,
    pub(crate) read_offset: usize,
    pub(crate) reported_discarded_bytes: u64,
    pub(crate) exhausted: bool,
    /// When set, `<process_id>.bin` lives under `source_thread_id`'s directory
    /// and is read-only for this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_thread_id: Option<ThreadId>,
    pub(crate) hook_command: Option<String>,
    pub(crate) tool_cwd: Option<PathUri>,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedRetainedEntry {
    pub(crate) meta: RetainedExecMeta,
    pub(crate) bin_path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct RetainedExecStorage {
    codex_home: PathBuf,
    thread_id: ThreadId,
    ephemeral: bool,
}

pub(crate) fn retained_exec_root(codex_home: &Path) -> PathBuf {
    codex_home.join(RETAINED_EXEC_SUBDIR)
}

pub(crate) fn retained_exec_thread_dir(codex_home: &Path, thread_id: ThreadId) -> PathBuf {
    retained_exec_root(codex_home).join(thread_id.to_string())
}

impl RetainedExecStorage {
    pub(crate) fn new(codex_home: PathBuf, thread_id: ThreadId, ephemeral: bool) -> Self {
        Self {
            codex_home,
            thread_id,
            ephemeral,
        }
    }

    pub(crate) fn thread_dir(&self) -> PathBuf {
        retained_exec_thread_dir(&self.codex_home, self.thread_id)
    }

    pub(crate) fn bin_path_for(
        &self,
        process_id: i32,
        source_thread_id: Option<ThreadId>,
    ) -> PathBuf {
        let owner = source_thread_id.unwrap_or(self.thread_id);
        retained_exec_thread_dir(&self.codex_home, owner).join(format!("{process_id}.bin"))
    }

    fn meta_path(&self, process_id: i32) -> PathBuf {
        self.thread_dir().join(format!("{process_id}.json"))
    }

    pub(crate) fn is_ephemeral(&self) -> bool {
        self.ephemeral
    }

    /// Load retained metadata for this thread (used on resume).
    /// Returns all reserved process IDs (including exhausted ones) plus all
    /// still-readable entries.
    pub(crate) fn load_own_entries(&self) -> (HashSet<i32>, Vec<LoadedRetainedEntry>) {
        let dir = self.thread_dir();
        let metas = scan_meta_dir(&dir);
        let mut reserved = HashSet::with_capacity(metas.len());
        let mut loaded = Vec::new();
        for meta in metas {
            reserved.insert(meta.process_id);
            if meta.exhausted {
                continue;
            }
            let bin_path = self.bin_path_for(meta.process_id, meta.source_thread_id);
            loaded.push(LoadedRetainedEntry { meta, bin_path });
        }
        (reserved, loaded)
    }

    /// Snapshot retained entries from `source_thread_id` for a forked thread.
    /// Never modifies or deletes any files in `source_thread_id`.
    pub(crate) fn load_fork_snapshot(
        &self,
        source_thread_id: ThreadId,
        already_loaded: &HashSet<i32>,
    ) -> (HashSet<i32>, Vec<LoadedRetainedEntry>) {
        let source_dir = retained_exec_thread_dir(&self.codex_home, source_thread_id);
        let metas = scan_meta_dir(&source_dir);
        let mut reserved = HashSet::with_capacity(metas.len());
        let mut loaded = Vec::new();
        for mut meta in metas {
            reserved.insert(meta.process_id);
            if already_loaded.contains(&meta.process_id) || meta.exhausted {
                continue;
            }
            let bin_owner = meta.source_thread_id.unwrap_or(source_thread_id);
            meta.source_thread_id = Some(bin_owner);
            meta.completed = true;
            let bin_path = self.bin_path_for(meta.process_id, Some(bin_owner));
            if !self.ephemeral
                && let Err(err) = self.persist_meta(&meta)
            {
                warn!(
                    thread_id = %self.thread_id,
                    process_id = meta.process_id,
                    "failed to persist fork retained exec snapshot: {err}"
                );
            }
            loaded.push(LoadedRetainedEntry { meta, bin_path });
        }
        (reserved, loaded)
    }

    /// Persist output bytes (appending only the newly arrived delta when possible)
    /// and atomically update `<process_id>.json`.
    pub(crate) fn persist_delta(
        &self,
        process_id: i32,
        previously_persisted_bytes: usize,
        all_bytes: &[u8],
        meta: &RetainedExecMeta,
    ) -> std::io::Result<usize> {
        if self.ephemeral {
            return Ok(all_bytes.len());
        }
        self.ensure_dir()?;
        let bin_path = self.bin_path_for(process_id, /*source_thread_id*/ None);
        let existing_len = fs::metadata(&bin_path)
            .ok()
            .and_then(|m| usize::try_from(m.len()).ok());
        if previously_persisted_bytes > 0
            && existing_len == Some(previously_persisted_bytes)
            && previously_persisted_bytes <= all_bytes.len()
        {
            if previously_persisted_bytes < all_bytes.len() {
                let mut file = open_private_append(&bin_path)?;
                file.write_all(&all_bytes[previously_persisted_bytes..])?;
            }
        } else {
            let mut file = open_private_truncate(&bin_path)?;
            file.write_all(all_bytes)?;
        }
        self.write_meta_atomic(meta)?;
        Ok(all_bytes.len())
    }

    /// Atomically persist metadata to `<codex_home>/retained_exec/<thread_id>/<process_id>.json`.
    pub(crate) fn persist_meta(&self, meta: &RetainedExecMeta) -> std::io::Result<()> {
        if self.ephemeral {
            return Ok(());
        }
        self.ensure_dir()?;
        self.write_meta_atomic(meta)
    }

    fn ensure_dir(&self) -> std::io::Result<()> {
        let root = retained_exec_root(&self.codex_home);
        create_private_dir(&root)?;
        let dir = self.thread_dir();
        create_private_dir(&dir)?;
        Ok(())
    }

    fn write_meta_atomic(&self, meta: &RetainedExecMeta) -> std::io::Result<()> {
        let final_path = self.meta_path(meta.process_id);
        let tmp_path =
            self.thread_dir()
                .join(format!("{}.json.tmp.{}", meta.process_id, Uuid::new_v4()));
        let serialized = serde_json::to_vec(meta)
            .map_err(|err| std::io::Error::new(ErrorKind::InvalidData, err))?;
        {
            let mut file = open_private_truncate(&tmp_path)?;
            file.write_all(&serialized)?;
        }
        if let Err(err) = fs::rename(&tmp_path, &final_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(())
    }
}

/// Read up to `max_len` bytes starting at `read_offset` without reading past `retained_bytes`.
pub(crate) fn read_retained_window(
    bin_path: &Path,
    read_offset: usize,
    retained_bytes: usize,
    max_len: usize,
) -> std::io::Result<Vec<u8>> {
    let available = retained_bytes.saturating_sub(read_offset);
    let to_read = available.min(max_len);
    if to_read == 0 {
        return Ok(Vec::new());
    }
    let mut file = File::open(bin_path)?;
    file.seek(SeekFrom::Start(read_offset as u64))?;
    let mut buf = vec![0u8; to_read];
    let mut filled = 0usize;
    while filled < to_read {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

fn scan_meta_dir(dir: &Path) -> Vec<RetainedExecMeta> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            warn!(dir = %dir.display(), "failed to read retained exec directory: {err}");
            return Vec::new();
        }
    };
    let mut metas = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read(&path) else {
            continue;
        };
        match serde_json::from_slice::<RetainedExecMeta>(&raw) {
            Ok(meta) => metas.push(meta),
            Err(err) => {
                warn!(path = %path.display(), "ignoring unreadable retained exec metadata: {err}");
            }
        }
    }
    metas
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn open_private_truncate(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn open_private_append(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

#[cfg(test)]
#[path = "retained_tests.rs"]
mod tests;
