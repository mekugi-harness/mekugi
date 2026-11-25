use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::error::ChangeHistoryError;
use crate::identity::ChangeId;
use crate::identity::UNATTRIBUTED_STREAM;
use crate::identity::stream_name_from_index;
use crate::model::CURRENT_SCHEMA_VERSION;
use crate::model::ChangeAuthor;
use crate::model::ChangeIndexEntry;
use crate::model::ChangeMetadata;
use crate::model::ChangeRecord;
use crate::model::ChangeScope;
use crate::model::FileChangeKind;
use crate::model::FileEvidence;
use crate::model::FileSnapshot;
use crate::model::FileState;
use crate::model::PendingFileChange;

const BLOBS_DIR: &str = "blobs";
const RECORDS_DIR: &str = "records";
const INDEX_FILE: &str = "index.json";
const LOCK_FILE: &str = "store.lock";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StoreIndex {
    schema_version: u32,
    next_sequence: u64,
    /// Maps `thread_id -> stream_name` (e.g. `"a"`, `"b"`).
    thread_streams: BTreeMap<String, String>,
    /// Maps `stream_name -> next_number` (1-based).
    stream_next: BTreeMap<String, u64>,
    /// Maps `claim_key -> ChangeId` to prevent double-counting overlapping patch/shell writes.
    claimed: BTreeMap<String, ChangeId>,
    /// Ordered list of published change entries.
    entries: Vec<ChangeIndexEntry>,
}

impl StoreIndex {
    fn new() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            next_sequence: 1,
            thread_streams: BTreeMap::new(),
            stream_next: BTreeMap::new(),
            claimed: BTreeMap::new(),
            entries: Vec::new(),
        }
    }
}

/// Shared durable per-session change-history store.
///
/// Uses cross-process file locking (`store.lock`) and atomic temp-file renames
/// to enforce persist-before-publish semantics: all content-addressed blobs and
/// the immutable change record are written and synced before `index.json`
/// publishes the change ID.
#[derive(Clone, Debug)]
pub struct ChangeHistoryStore {
    root: PathBuf,
}

impl ChangeHistoryStore {
    /// Open or initialize a change-history store at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ChangeHistoryError> {
        let root = root.into();
        fs::create_dir_all(root.join(BLOBS_DIR)).map_err(|err| {
            ChangeHistoryError::Storage(format!(
                "create blobs dir {}: {err}",
                root.join(BLOBS_DIR).display()
            ))
        })?;
        fs::create_dir_all(root.join(RECORDS_DIR)).map_err(|err| {
            ChangeHistoryError::Storage(format!(
                "create records dir {}: {err}",
                root.join(RECORDS_DIR).display()
            ))
        })?;
        Ok(Self { root })
    }

    /// Open an existing store without creating missing directories.
    pub fn open_existing(root: impl Into<PathBuf>) -> Result<Self, ChangeHistoryError> {
        let root = root.into();
        if !root.is_dir() {
            return Err(ChangeHistoryError::Storage(format!(
                "change-history store does not exist: {}",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Persist all file baselines/snapshots and publish a new [`ChangeRecord`].
    ///
    /// If `metadata.claim_key` (or the default `(thread_id, call_id)` key when
    /// `call_id` is non-empty) has already been published, returns the existing
    /// record without double-counting.
    ///
    /// Persist-before-publish: if any blob or record write fails, `index.json`
    /// is untouched and the error is returned explicitly.
    pub fn record_change(
        &self,
        mut metadata: ChangeMetadata,
        pending_files: Vec<PendingFileChange>,
    ) -> Result<ChangeRecord, ChangeHistoryError> {
        let _lock = self.acquire_exclusive_lock()?;
        let mut index = self.read_index_locked()?;

        let effective_claim = effective_claim_key(&metadata);
        if let Some(claim_key) = &effective_claim
            && let Some(existing_id) = index.claimed.get(claim_key).cloned()
        {
            return self.read_record_unlocked(&existing_id);
        }

        // Step 1: Persist all immutable before/after blobs BEFORE allocating or publishing.
        let mut files = Vec::with_capacity(pending_files.len());
        for pending in pending_files {
            let before = self.persist_snapshot(&pending.before)?;
            let after = self.persist_snapshot(&pending.after)?;
            let kind = classify_change_kind(&pending.moved_from, &before, &after);
            files.push(FileEvidence {
                rel_path: pending.rel_path,
                abs_path: pending.abs_path,
                moved_from: pending.moved_from,
                kind,
                before,
                after,
                coverage: pending.coverage,
                scope: pending.scope,
            });
        }

        // Step 2: Allocate stream and monotonic ChangeId in memory.
        let author = metadata.effective_author();
        let (stream, next_num, id) = if author.is_unattributed() {
            metadata.thread_id.clear();
            metadata.author = Some(ChangeAuthor::Unattributed);
            let next_num = *index.stream_next.get(UNATTRIBUTED_STREAM).unwrap_or(&1);
            let id = ChangeId::new_unattributed(next_num)?;
            (UNATTRIBUTED_STREAM.to_string(), next_num, id)
        } else {
            metadata.author = Some(author);
            let stream = if let Some(existing) = index.thread_streams.get(&metadata.thread_id) {
                existing.clone()
            } else {
                let next_stream = stream_name_from_index(index.thread_streams.len());
                index
                    .thread_streams
                    .insert(metadata.thread_id.clone(), next_stream.clone());
                next_stream
            };
            let next_num = *index.stream_next.get(&stream).unwrap_or(&1);
            let id = ChangeId::new(&stream, next_num)?;
            (stream, next_num, id)
        };
        let sequence = index.next_sequence;

        let record = ChangeRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: id.clone(),
            sequence,
            metadata,
            files,
        };

        // Step 3: Atomically persist the immutable record file BEFORE updating index.json.
        let record_path = self.record_path(&id);
        let record_bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| ChangeHistoryError::Storage(format!("serialize record {id}: {err}")))?;
        atomic_write(&record_path, &record_bytes)?;

        // Step 4: Update and atomically publish index.json.
        index.stream_next.insert(stream, next_num + 1);
        index.next_sequence = sequence + 1;
        if let Some(claim_key) = effective_claim {
            index.claimed.insert(claim_key, id.clone());
        }
        let file_paths = record.files.iter().map(|f| f.rel_path.clone()).collect();
        let has_incomplete_coverage = record.files.iter().any(|f| !f.coverage.is_complete());
        let ephemeral_only = !record.files.is_empty()
            && record
                .files
                .iter()
                .all(|f| f.scope == ChangeScope::Ephemeral);
        index.entries.push(ChangeIndexEntry {
            id,
            sequence,
            thread_id: record.metadata.thread_id.clone(),
            author: record.metadata.author.clone(),
            call_id: record.metadata.call_id.clone(),
            source_kind: record.metadata.source_kind,
            created_at: record.metadata.created_at,
            exit_code: record.metadata.exit_code,
            file_paths,
            has_incomplete_coverage,
            ephemeral_only,
        });
        self.write_index_locked(&index)?;

        Ok(record)
    }

    /// Check whether a claim key has already been recorded in this store.
    pub fn claimed_change_id(
        &self,
        claim_key: &str,
    ) -> Result<Option<ChangeId>, ChangeHistoryError> {
        let _lock = self.acquire_shared_lock()?;
        let index = self.read_index_locked()?;
        Ok(index.claimed.get(claim_key).cloned())
    }

    /// List all published change index entries in capture order.
    pub fn list_entries(&self) -> Result<Vec<ChangeIndexEntry>, ChangeHistoryError> {
        let _lock = self.acquire_shared_lock()?;
        let index = self.read_index_locked()?;
        Ok(index.entries)
    }

    /// Load a single [`ChangeRecord`] by ID, verifying its presence and JSON integrity.
    pub fn get_record(&self, id: &ChangeId) -> Result<ChangeRecord, ChangeHistoryError> {
        let _lock = self.acquire_shared_lock()?;
        let index = self.read_index_locked()?;
        if !index.entries.iter().any(|entry| &entry.id == id) {
            return Err(ChangeHistoryError::UnknownChangeId(id.to_string()));
        }
        self.read_record_unlocked(id)
    }

    /// Load multiple records by IDs in capture order, failing explicitly if any ID
    /// is unknown, missing, or corrupt.
    pub fn load_records(&self, ids: &[ChangeId]) -> Result<Vec<ChangeRecord>, ChangeHistoryError> {
        if ids.is_empty() {
            return Err(ChangeHistoryError::EmptySelection);
        }
        let _lock = self.acquire_shared_lock()?;
        let index = self.read_index_locked()?;
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            if !index.entries.iter().any(|entry| &entry.id == id) {
                return Err(ChangeHistoryError::UnknownChangeId(id.to_string()));
            }
            records.push(self.read_record_unlocked(id)?);
        }
        records.sort_by_key(|record| record.sequence);
        records.dedup_by(|a, b| a.id == b.id);
        Ok(records)
    }

    /// Read and integrity-check a persisted [`FileState`] back into raw bytes.
    pub fn read_file_state(
        &self,
        record_id: &ChangeId,
        state: &FileState,
    ) -> Result<FileSnapshot, ChangeHistoryError> {
        match state {
            FileState::Missing => Ok(FileSnapshot::Missing),
            FileState::Present {
                blob_hash,
                byte_len,
                ..
            } => {
                let bytes = self.read_blob_verified(record_id, blob_hash)?;
                if bytes.len() as u64 != *byte_len {
                    return Err(ChangeHistoryError::CorruptEvidence {
                        id: record_id.to_string(),
                        detail: format!(
                            "blob {blob_hash} length mismatch (expected {byte_len}, got {})",
                            bytes.len()
                        ),
                    });
                }
                Ok(FileSnapshot::Present(bytes))
            }
        }
    }

    fn read_blob_verified(
        &self,
        record_id: &ChangeId,
        blob_hash: &str,
    ) -> Result<Vec<u8>, ChangeHistoryError> {
        if !is_valid_sha256_hex(blob_hash) {
            return Err(ChangeHistoryError::CorruptEvidence {
                id: record_id.to_string(),
                detail: format!("invalid blob hash `{blob_hash}`"),
            });
        }
        let path = self.blob_path(blob_hash);
        let bytes = fs::read(&path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ChangeHistoryError::MissingEvidence {
                    id: record_id.to_string(),
                    detail: format!("missing blob {blob_hash}"),
                }
            } else {
                ChangeHistoryError::Storage(format!("read blob {}: {err}", path.display()))
            }
        })?;
        let actual = sha256_hex(&bytes);
        if actual != blob_hash {
            return Err(ChangeHistoryError::CorruptEvidence {
                id: record_id.to_string(),
                detail: format!("blob {blob_hash} checksum mismatch (got {actual})"),
            });
        }
        Ok(bytes)
    }

    fn persist_snapshot(&self, snapshot: &FileSnapshot) -> Result<FileState, ChangeHistoryError> {
        match snapshot {
            FileSnapshot::Missing => Ok(FileState::Missing),
            FileSnapshot::Present(bytes) => {
                let hash = sha256_hex(bytes);
                let path = self.blob_path(&hash);
                if !path.exists() {
                    atomic_write(&path, bytes)?;
                }
                Ok(FileState::Present {
                    blob_hash: hash,
                    byte_len: bytes.len() as u64,
                    is_binary: FileSnapshot::is_binary(bytes),
                })
            }
        }
    }

    fn read_record_unlocked(&self, id: &ChangeId) -> Result<ChangeRecord, ChangeHistoryError> {
        let path = self.record_path(id);
        let bytes = fs::read(&path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ChangeHistoryError::MissingEvidence {
                    id: id.to_string(),
                    detail: format!("missing record file {}", path.display()),
                }
            } else {
                ChangeHistoryError::Storage(format!("read record {}: {err}", path.display()))
            }
        })?;
        let record: ChangeRecord =
            serde_json::from_slice(&bytes).map_err(|err| ChangeHistoryError::CorruptEvidence {
                id: id.to_string(),
                detail: format!("corrupt record JSON: {err}"),
            })?;
        if &record.id != id {
            return Err(ChangeHistoryError::CorruptEvidence {
                id: id.to_string(),
                detail: format!("record ID mismatch (file holds `{}`)", record.id),
            });
        }
        Ok(record)
    }

    fn read_index_locked(&self) -> Result<StoreIndex, ChangeHistoryError> {
        let path = self.root.join(INDEX_FILE);
        match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|err| ChangeHistoryError::CorruptEvidence {
                    id: INDEX_FILE.to_string(),
                    detail: format!("corrupt store index: {err}"),
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(StoreIndex::new()),
            Err(err) => Err(ChangeHistoryError::Storage(format!(
                "read index {}: {err}",
                path.display()
            ))),
        }
    }

    fn write_index_locked(&self, index: &StoreIndex) -> Result<(), ChangeHistoryError> {
        let path = self.root.join(INDEX_FILE);
        let bytes = serde_json::to_vec_pretty(index)
            .map_err(|err| ChangeHistoryError::Storage(format!("serialize index: {err}")))?;
        atomic_write(&path, &bytes)
    }

    fn acquire_exclusive_lock(&self) -> Result<File, ChangeHistoryError> {
        let lock_file = self.open_lock_file()?;
        lock_file
            .lock()
            .map_err(|err| ChangeHistoryError::Storage(format!("lock store exclusively: {err}")))?;
        Ok(lock_file)
    }

    fn acquire_shared_lock(&self) -> Result<File, ChangeHistoryError> {
        let lock_file = self.open_lock_file()?;
        lock_file
            .lock_shared()
            .map_err(|err| ChangeHistoryError::Storage(format!("lock store shared: {err}")))?;
        Ok(lock_file)
    }

    fn open_lock_file(&self) -> Result<File, ChangeHistoryError> {
        let path = self.root.join(LOCK_FILE);
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| {
                ChangeHistoryError::Storage(format!("open lock file {}: {err}", path.display()))
            })
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.root.join(BLOBS_DIR).join(hash)
    }

    fn record_path(&self, id: &ChangeId) -> PathBuf {
        self.root.join(RECORDS_DIR).join(format!("{id}.json"))
    }
}

fn effective_claim_key(metadata: &ChangeMetadata) -> Option<String> {
    if let Some(claim) = &metadata.claim_key
        && !claim.is_empty()
    {
        return Some(claim.clone());
    }
    if !metadata.call_id.is_empty() {
        return Some(format!("call:{}:{}", metadata.thread_id, metadata.call_id));
    }
    None
}

fn classify_change_kind(
    moved_from: &Option<String>,
    before: &FileState,
    after: &FileState,
) -> FileChangeKind {
    if let Some(from) = moved_from {
        return FileChangeKind::Move { from: from.clone() };
    }
    match (before, after) {
        (FileState::Missing, FileState::Present { .. }) => FileChangeKind::Add,
        (FileState::Present { .. }, FileState::Missing) => FileChangeKind::Delete,
        (FileState::Present { .. }, FileState::Present { .. })
        | (FileState::Missing, FileState::Missing) => FileChangeKind::Modify,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ChangeHistoryError> {
    let parent = path.parent().ok_or_else(|| {
        ChangeHistoryError::Storage(format!("no parent directory for {}", path.display()))
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|err| {
        ChangeHistoryError::Storage(format!("create temp file in {}: {err}", parent.display()))
    })?;
    tmp.write_all(bytes)
        .and_then(|()| tmp.as_file().sync_all())
        .map_err(|err| {
            ChangeHistoryError::Storage(format!("write temp file for {}: {err}", path.display()))
        })?;
    tmp.persist(path).map_err(|err| {
        ChangeHistoryError::Storage(format!("persist {}: {}", path.display(), err.error))
    })?;
    Ok(())
}
