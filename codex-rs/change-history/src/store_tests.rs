use std::fs;
use std::path::Path;
use std::str::FromStr;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::ChangeHistoryError;
use crate::ChangeHistoryStore;
use crate::ChangeId;
use crate::ChangeMetadata;
use crate::ChangeSourceKind;
use crate::CoverageStatus;
use crate::FileSnapshot;
use crate::FileState;
use crate::MutationDirection;
use crate::PendingFileChange;
use crate::compose_diff;
use crate::compose_summary;
use crate::expand_id_spec;
use crate::stream_name_from_index;

fn test_metadata(
    workspace: &Path,
    thread_id: &str,
    call_id: &str,
    source_kind: ChangeSourceKind,
) -> ChangeMetadata {
    ChangeMetadata {
        thread_id: thread_id.to_string(),
        session_id: "sess-1".to_string(),
        turn_id: "turn-1".to_string(),
        call_id: call_id.to_string(),
        source_kind,
        workspace_root: workspace.to_path_buf(),
        created_at: 1_700_000_000,
        exit_code: Some(0),
        summary: None,
        claim_key: None,
    }
}

#[test]
fn identity_parsing_and_range_expansion() {
    assert_eq!(stream_name_from_index(0), "a");
    assert_eq!(stream_name_from_index(1), "b");
    assert_eq!(stream_name_from_index(25), "z");
    assert_eq!(stream_name_from_index(26), "aa");

    let id = ChangeId::new("a", 1).expect("valid id");
    assert_eq!(id.as_str(), "amber1");
    assert_eq!(ChangeId::from_str("amber1").expect("parse"), id);

    let range = expand_id_spec("amber1..3").expect("abbreviated range");
    assert_eq!(
        range.iter().map(ChangeId::as_str).collect::<Vec<_>>(),
        vec!["amber1", "amber2", "amber3"]
    );

    let full_range = expand_id_spec("apple2..apple4").expect("full range");
    assert_eq!(
        full_range.iter().map(ChangeId::as_str).collect::<Vec<_>>(),
        vec!["apple2", "apple3", "apple4"]
    );

    assert!(expand_id_spec("amber1..apple2").is_err());
    assert!(expand_id_spec("amber3..1").is_err());
    assert!(ChangeId::from_str("amber0").is_err());
    assert!(ChangeId::from_str("unknown1").is_err());
}

#[test]
fn monotonic_ids_per_thread_and_claim_deduplication() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(dir.path().join("store")).expect("open");

    let r1 = store
        .record_change(
            test_metadata(&ws, "thread-a", "call-1", ChangeSourceKind::ApplyPatch),
            vec![PendingFileChange {
                rel_path: "a.txt".to_string(),
                abs_path: ws.join("a.txt"),
                moved_from: None,
                before: FileSnapshot::Missing,
                after: FileSnapshot::Present(b"hello\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("record r1");
    assert_eq!(r1.id.as_str(), "amber1");

    // Duplicate recording for the same (thread, call_id) must not double-count.
    let dup = store
        .record_change(
            test_metadata(&ws, "thread-a", "call-1", ChangeSourceKind::ShellWrite),
            vec![PendingFileChange {
                rel_path: "a.txt".to_string(),
                abs_path: ws.join("a.txt"),
                moved_from: None,
                before: FileSnapshot::Missing,
                after: FileSnapshot::Present(b"hello\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("dedup record");
    assert_eq!(dup.id, r1.id);
    assert_eq!(store.list_entries().expect("list").len(), 1);

    // Second thread gets its own stream (`apple1`).
    let r2 = store
        .record_change(
            test_metadata(&ws, "thread-b", "call-2", ChangeSourceKind::ApplyPatch),
            vec![PendingFileChange {
                rel_path: "b.txt".to_string(),
                abs_path: ws.join("b.txt"),
                moved_from: None,
                before: FileSnapshot::Missing,
                after: FileSnapshot::Present(b"world\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("record r2");
    assert_eq!(r2.id.as_str(), "apple1");
}

#[test]
fn missing_or_corrupt_blob_fails_explicitly() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(dir.path().join("store")).expect("open");

    let rec = store
        .record_change(
            test_metadata(&ws, "thread-a", "call-1", ChangeSourceKind::ApplyPatch),
            vec![PendingFileChange {
                rel_path: "a.txt".to_string(),
                abs_path: ws.join("a.txt"),
                moved_from: None,
                before: FileSnapshot::Missing,
                after: FileSnapshot::Present(b"original\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("record");

    let blob_hash = match &rec.files[0].after {
        FileState::Present { blob_hash, .. } => blob_hash.clone(),
        FileState::Missing => panic!("expected present state"),
    };
    let blob_path = store.root().join("blobs").join(&blob_hash);

    // Corrupt the blob bytes.
    fs::write(&blob_path, b"tampered\n").expect("corrupt");
    let err = compose_diff(&store, std::slice::from_ref(&rec), &[]).expect_err("must fail");
    assert!(matches!(err, ChangeHistoryError::CorruptEvidence { .. }));

    // Remove the blob file entirely.
    fs::remove_file(&blob_path).expect("remove");
    let err = compose_diff(&store, std::slice::from_ref(&rec), &[]).expect_err("must fail");
    assert!(matches!(err, ChangeHistoryError::MissingEvidence { .. }));
}

#[test]
fn composed_diff_and_summary_across_multiple_records() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(dir.path().join("store")).expect("open");

    let r1 = store
        .record_change(
            test_metadata(&ws, "t1", "c1", ChangeSourceKind::ApplyPatch),
            vec![PendingFileChange {
                rel_path: "f.txt".to_string(),
                abs_path: ws.join("f.txt"),
                moved_from: None,
                before: FileSnapshot::Present(b"line1\nline2\n".to_vec()),
                after: FileSnapshot::Present(b"line1\nline2-v2\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("r1");
    let r2 = store
        .record_change(
            test_metadata(&ws, "t1", "c2", ChangeSourceKind::ApplyPatch),
            vec![PendingFileChange {
                rel_path: "f.txt".to_string(),
                abs_path: ws.join("f.txt"),
                moved_from: None,
                before: FileSnapshot::Present(b"line1\nline2-v2\n".to_vec()),
                after: FileSnapshot::Present(b"line1\nline2-v3\nline3\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("r2");

    let summary = compose_summary(&store, &[r1.clone(), r2.clone()], &[]).expect("summary");
    assert_eq!(summary.files.len(), 1);
    assert_eq!(summary.files[0].status, 'M');
    assert_eq!(summary.total_additions, 2);
    assert_eq!(summary.total_deletions, 1);

    let diffs = compose_diff(&store, &[r1, r2], &[]).expect("diff");
    assert!(diffs[0].unified_diff.contains("-line2\n"));
    assert!(diffs[0].unified_diff.contains("+line2-v3\n"));
    assert!(diffs[0].unified_diff.contains("+line3\n"));
}

#[test]
fn path_scoped_revert_preserves_unrelated_edits_and_fails_on_drift_without_markers() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(dir.path().join("store")).expect("open");

    let a_path = ws.join("a.txt");
    let b_path = ws.join("b.txt");
    let a_before = b"alpha\nbeta\ngamma\ndelta\n";
    let a_after = b"alpha-edited\nbeta\ngamma\ndelta\n";
    let b_before = b"one\ntwo\n";
    let b_after = b"one\ntwo-edited\n";

    let rec = store
        .record_change(
            test_metadata(&ws, "t1", "c1", ChangeSourceKind::ApplyPatch),
            vec![
                PendingFileChange {
                    rel_path: "a.txt".to_string(),
                    abs_path: a_path.clone(),
                    moved_from: None,
                    before: FileSnapshot::Present(a_before.to_vec()),
                    after: FileSnapshot::Present(a_after.to_vec()),
                    coverage: CoverageStatus::Complete,
                },
                PendingFileChange {
                    rel_path: "b.txt".to_string(),
                    abs_path: b_path.clone(),
                    moved_from: None,
                    before: FileSnapshot::Present(b_before.to_vec()),
                    after: FileSnapshot::Present(b_after.to_vec()),
                    coverage: CoverageStatus::Complete,
                },
            ],
        )
        .expect("record");

    // Add an unrelated edit on the last line of `a.txt` and keep `b.txt` edited.
    fs::write(&a_path, b"alpha-edited\nbeta\ngamma\ndelta-unrelated\n").expect("write a");
    fs::write(&b_path, b_after).expect("write b");

    // Path-scoped revert of only `a.txt` must revert line 1, preserve line 4, and leave `b.txt` untouched.
    let outcome = store
        .mutate_workspace(
            MutationDirection::Revert,
            std::slice::from_ref(&rec.id),
            &["a.txt".to_string()],
            "t1",
            "sess-1",
        )
        .expect("revert a.txt");
    assert_eq!(outcome.affected_paths, vec!["a.txt"]);
    assert_eq!(
        fs::read_to_string(&a_path).expect("read a"),
        "alpha\nbeta\ngamma\ndelta-unrelated\n"
    );
    assert_eq!(fs::read(&b_path).expect("read b"), b_after);

    // Reapply `a.txt` restores `alpha-edited` while still preserving `delta-unrelated`.
    store
        .mutate_workspace(
            MutationDirection::Apply,
            std::slice::from_ref(&rec.id),
            &["a.txt".to_string()],
            "t1",
            "sess-1",
        )
        .expect("reapply a.txt");
    assert_eq!(
        fs::read_to_string(&a_path).expect("read a"),
        "alpha-edited\nbeta\ngamma\ndelta-unrelated\n"
    );

    // Overlapping drift on line 1 must fail explicitly without writing conflict markers.
    let drifted = "alpha-CONFLICTING\nbeta\ngamma\ndelta-unrelated\n";
    fs::write(&a_path, drifted).expect("write drifted");
    let err = store
        .mutate_workspace(
            MutationDirection::Revert,
            std::slice::from_ref(&rec.id),
            &["a.txt".to_string()],
            "t1",
            "sess-1",
        )
        .expect_err("must reject overlapping drift");
    assert!(matches!(err, ChangeHistoryError::DriftConflict { .. }));
    let on_disk = fs::read_to_string(&a_path).expect("read after failed revert");
    assert_eq!(on_disk, drifted);
    assert!(!on_disk.contains("<<<<<<<"));
}

#[test]
fn revert_and_apply_handle_creation_deletion_and_deletion_restoration() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(dir.path().join("store")).expect("open");

    let created_path = ws.join("new.txt");
    let deleted_path = ws.join("old.txt");

    let rec = store
        .record_change(
            test_metadata(&ws, "t1", "c1", ChangeSourceKind::ApplyPatch),
            vec![
                PendingFileChange {
                    rel_path: "new.txt".to_string(),
                    abs_path: created_path.clone(),
                    moved_from: None,
                    before: FileSnapshot::Missing,
                    after: FileSnapshot::Present(b"created\n".to_vec()),
                    coverage: CoverageStatus::Complete,
                },
                PendingFileChange {
                    rel_path: "old.txt".to_string(),
                    abs_path: deleted_path.clone(),
                    moved_from: None,
                    before: FileSnapshot::Present(b"deleted\n".to_vec()),
                    after: FileSnapshot::Missing,
                    coverage: CoverageStatus::Complete,
                },
            ],
        )
        .expect("record");

    fs::write(&created_path, b"created\n").expect("write created");
    assert!(!deleted_path.exists());

    // Revert: deletes created file and restores deleted file.
    store
        .mutate_workspace(
            MutationDirection::Revert,
            std::slice::from_ref(&rec.id),
            &[],
            "t1",
            "sess-1",
        )
        .expect("revert");
    assert!(!created_path.exists());
    assert_eq!(
        fs::read_to_string(&deleted_path).expect("restored"),
        "deleted\n"
    );

    // Reapply: recreates new.txt and deletes old.txt.
    store
        .mutate_workspace(
            MutationDirection::Apply,
            std::slice::from_ref(&rec.id),
            &[],
            "t1",
            "sess-1",
        )
        .expect("apply");
    assert_eq!(
        fs::read_to_string(&created_path).expect("recreated"),
        "created\n"
    );
    assert!(!deleted_path.exists());
}

#[test]
fn persist_before_publish_leaves_index_clean_on_storage_failure() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");
    let store_dir = dir.path().join("store");
    let store = ChangeHistoryStore::open(&store_dir).expect("open");

    // Replace blobs/ directory with a regular file so blob persistence fails.
    fs::remove_dir_all(store_dir.join("blobs")).expect("remove blobs dir");
    fs::write(store_dir.join("blobs"), b"not a directory").expect("block blobs");

    let err = store
        .record_change(
            test_metadata(&ws, "t1", "c1", ChangeSourceKind::ApplyPatch),
            vec![PendingFileChange {
                rel_path: "a.txt".to_string(),
                abs_path: ws.join("a.txt"),
                moved_from: None,
                before: FileSnapshot::Missing,
                after: FileSnapshot::Present(b"hello\n".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect_err("storage failure must surface explicitly");
    assert!(matches!(err, ChangeHistoryError::Storage(_)));
    // Nothing was published to index.json.
    assert!(store.list_entries().expect("list").is_empty());
}
