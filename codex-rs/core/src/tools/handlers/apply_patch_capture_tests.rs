use std::fs;

use codex_change_history::ChangeHistoryError;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeSourceKind;
use codex_change_history::FileSnapshot;
use codex_change_history::compose_diff;
use codex_exec_server::LOCAL_FS;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::ApplyPatchCapture;

#[tokio::test]
async fn captures_pre_and_post_bytes_including_partial_writes_on_failure() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    let store_dir = dir.path().join("store");
    fs::create_dir_all(&ws).expect("ws");

    let first = ws.join("first.txt");
    let second = ws.join("second.txt");
    fs::write(&first, b"before-1\n").expect("write first");

    let first_uri = PathUri::from(AbsolutePathBuf::try_from(first.clone()).expect("abs first"));
    let second_uri = PathUri::from(AbsolutePathBuf::try_from(second.clone()).expect("abs second"));
    let candidates = vec![first_uri, second_uri];

    let capture = ApplyPatchCapture::capture_baselines(LOCAL_FS.as_ref(), &candidates).await;

    // Simulate a patch that mutates first.txt before failing on second.txt (exit_code = 1).
    fs::write(&first, b"after-partial-1\n").expect("mutate first");

    let record = capture
        .persist_to_dir(
            LOCAL_FS.as_ref(),
            &store_dir,
            &candidates,
            /*delta*/ None,
            ChangeMetadata {
                thread_id: "thread-1".to_string(),
                session_id: "sess-1".to_string(),
                turn_id: "turn-1".to_string(),
                call_id: "call-partial".to_string(),
                source_kind: ChangeSourceKind::ApplyPatch,
                workspace_root: ws.clone(),
                created_at: 1_700_000_000,
                exit_code: Some(1),
                summary: Some("apply_patch".to_string()),
                claim_key: None,
            },
        )
        .await
        .expect("persist partial write")
        .expect("record created");

    assert_eq!(record.id.as_str(), "amber1");
    assert_eq!(record.metadata.exit_code, Some(1));
    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].rel_path, "first.txt");

    let store = ChangeHistoryStore::open_existing(&store_dir).expect("open store");
    assert_eq!(
        store
            .read_file_state(&record.id, &record.files[0].before)
            .expect("before"),
        FileSnapshot::Present(b"before-1\n".to_vec())
    );
    assert_eq!(
        store
            .read_file_state(&record.id, &record.files[0].after)
            .expect("after"),
        FileSnapshot::Present(b"after-partial-1\n".to_vec())
    );

    let diffs = compose_diff(&store, &[record], &[]).expect("diff");
    assert!(diffs[0].unified_diff.contains("-before-1\n"));
    assert!(diffs[0].unified_diff.contains("+after-partial-1\n"));
}

#[tokio::test]
async fn storage_failure_surfaces_explicitly_without_publishing() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    fs::create_dir_all(&ws).expect("ws");

    // Point store_dir at a regular file so ChangeHistoryStore::open fails.
    let blocked_store = dir.path().join("not-a-dir");
    fs::write(&blocked_store, b"blocked").expect("write blocker");

    let target = ws.join("a.txt");
    let target_uri = PathUri::from(AbsolutePathBuf::try_from(target.clone()).expect("abs"));
    let candidates = vec![target_uri];

    let capture = ApplyPatchCapture::capture_baselines(LOCAL_FS.as_ref(), &candidates).await;
    fs::write(&target, b"created\n").expect("write target");

    let err = capture
        .persist_to_dir(
            LOCAL_FS.as_ref(),
            &blocked_store,
            &candidates,
            /*delta*/ None,
            ChangeMetadata {
                thread_id: "thread-1".to_string(),
                session_id: "sess-1".to_string(),
                turn_id: "turn-1".to_string(),
                call_id: "call-fail".to_string(),
                source_kind: ChangeSourceKind::ApplyPatch,
                workspace_root: ws,
                created_at: 1_700_000_000,
                exit_code: Some(0),
                summary: Some("apply_patch".to_string()),
                claim_key: None,
            },
        )
        .await
        .expect_err("storage failure must surface explicitly");
    assert!(matches!(err, ChangeHistoryError::Storage(_)));
}
