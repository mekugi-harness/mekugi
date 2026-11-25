//! Process-global active-writer registry to detect concurrent writes to the
//! exact same file (including newly created files that have no inode yet)
//! by overlapping agent commands in the same worktree.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum WriterKey {
    Path(PathBuf),
    Inode { dev: u64, ino: u64 },
}

impl WriterKey {
    pub(crate) fn keys_for_path(canonical_path: &Path) -> Vec<Self> {
        let mut keys = Vec::with_capacity(2);
        keys.push(Self::Path(canonical_path.to_path_buf()));
        #[cfg(unix)]
        if let Ok(meta) = fs::symlink_metadata(canonical_path) {
            use std::os::unix::fs::MetadataExt;
            keys.push(Self::Inode {
                dev: meta.dev(),
                ino: meta.ino(),
            });
        }
        keys
    }
}

struct RegistryState {
    by_key: HashMap<WriterKey, HashSet<String>>,
    overlapped: HashSet<(String, WriterKey)>,
}

fn registry() -> &'static Mutex<RegistryState> {
    static REG: OnceLock<Mutex<RegistryState>> = OnceLock::new();
    REG.get_or_init(|| {
        Mutex::new(RegistryState {
            by_key: HashMap::new(),
            overlapped: HashSet::new(),
        })
    })
}

/// Register that `call_id` is about to write `keys` (canonical path + optional inode).
/// Returns `true` if another active call is concurrently writing any of the same keys.
pub(crate) fn register_write(call_id: &str, keys: &[WriterKey]) -> bool {
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut any_overlap = false;
    for key in keys {
        let writers = state.by_key.entry(key.clone()).or_default();
        let existing_others: Vec<String> = writers
            .iter()
            .filter(|other| other.as_str() != call_id)
            .cloned()
            .collect();
        writers.insert(call_id.to_string());
        if !existing_others.is_empty() {
            any_overlap = true;
            state.overlapped.insert((call_id.to_string(), key.clone()));
            for other in existing_others {
                state.overlapped.insert((other, key.clone()));
            }
        }
    }
    any_overlap
}

/// Check whether `call_id` experienced an overlapping writer on any of `keys` and
/// unregister all keys held by `call_id`.
pub(crate) fn finish_call(call_id: &str, keys: &[WriterKey]) -> HashSet<WriterKey> {
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut conflicted = HashSet::new();
    for key in keys {
        if state.overlapped.remove(&(call_id.to_string(), key.clone())) {
            conflicted.insert(key.clone());
        }
        if let Some(writers) = state.by_key.get_mut(key) {
            writers.remove(call_id);
            if writers.is_empty() {
                state.by_key.remove(key);
            }
        }
    }
    conflicted
}
