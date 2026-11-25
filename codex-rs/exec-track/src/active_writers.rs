//! Process-global active-inode registry to detect concurrent writes to the exact
//! same file by overlapping agent commands in the same worktree.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct InodeKey {
    dev: u64,
    ino: u64,
}

impl InodeKey {
    #[cfg(unix)]
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(path).ok()?;
        Some(Self {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }

    #[cfg(not(unix))]
    pub(crate) fn from_path(_path: &Path) -> Option<Self> {
        None
    }
}

struct RegistryState {
    /// Active call_ids currently holding a write observation on each inode.
    by_inode: HashMap<InodeKey, HashSet<String>>,
    /// Inodes that experienced overlapping writers during an active call.
    overlapped: HashSet<(String, InodeKey)>,
}

fn registry() -> &'static Mutex<RegistryState> {
    static REG: OnceLock<Mutex<RegistryState>> = OnceLock::new();
    REG.get_or_init(|| {
        Mutex::new(RegistryState {
            by_inode: HashMap::new(),
            overlapped: HashSet::new(),
        })
    })
}

/// Register that `call_id` is about to write `inode`. Returns `true` if another
/// active call is concurrently writing the same inode.
pub(crate) fn register_write(call_id: &str, inode: InodeKey) -> bool {
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let writers = state.by_inode.entry(inode).or_default();
    let existing_others: Vec<String> = writers
        .iter()
        .filter(|other| other.as_str() != call_id)
        .cloned()
        .collect();
    writers.insert(call_id.to_string());
    let overlapped = !existing_others.is_empty();
    if overlapped {
        state.overlapped.insert((call_id.to_string(), inode));
        for other in existing_others {
            state.overlapped.insert((other, inode));
        }
    }
    overlapped
}

/// Check whether `call_id` experienced an overlapping writer on `inode` and
/// unregister all inodes held by `call_id`.
pub(crate) fn finish_call(call_id: &str, inodes: &[InodeKey]) -> HashSet<InodeKey> {
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut conflicted = HashSet::new();
    for &inode in inodes {
        if state.overlapped.remove(&(call_id.to_string(), inode)) {
            conflicted.insert(inode);
        }
        if let Some(writers) = state.by_inode.get_mut(&inode) {
            writers.remove(call_id);
            if writers.is_empty() {
                state.by_inode.remove(&inode);
            }
        }
    }
    conflicted
}
