use std::fs;

use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::FileState;
use codex_change_history::PendingFileChange;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::run_mchanges;

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn list_summary_diff_history_and_path_scoped_revert_apply() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    let store_dir = dir.path().join("store");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(&store_dir).expect("open");

    let a_path = ws.join("a.txt");
    let b_path = ws.join("b.txt");
    let r1 = store
        .record_change(
            ChangeMetadata {
                thread_id: "thread-1".to_string(),
                session_id: "sess-1".to_string(),
                turn_id: "turn-1".to_string(),
                call_id: "call-1".to_string(),
                source_kind: ChangeSourceKind::ApplyPatch,
                workspace_root: ws,
                created_at: 1_700_000_000,
                exit_code: Some(0),
                summary: Some("update a and b".to_string()),
                claim_key: None,
            },
            vec![
                PendingFileChange {
                    rel_path: "a.txt".to_string(),
                    abs_path: a_path.clone(),
                    moved_from: None,
                    before: FileSnapshot::Present(b"line1\nline2\nline3\n".to_vec()),
                    after: FileSnapshot::Present(b"line1-mod\nline2\nline3\n".to_vec()),
                    coverage: CoverageStatus::Complete,
                },
                PendingFileChange {
                    rel_path: "b.txt".to_string(),
                    abs_path: b_path.clone(),
                    moved_from: None,
                    before: FileSnapshot::Missing,
                    after: FileSnapshot::Present(b"brand-new\n".to_vec()),
                    coverage: CoverageStatus::Complete,
                },
            ],
        )
        .expect("record r1");
    assert_eq!(r1.id.as_str(), "amber1");

    let store_str = store_dir.to_string_lossy().into_owned();

    // --list
    let list_out = run_mchanges(&args(&["--store", &store_str, "--list"]));
    assert_eq!(list_out.exit_code, 0);
    assert!(list_out.stdout.contains("amber1"));
    assert!(list_out.stdout.contains("apply_patch"));

    // --summary
    let sum_out = run_mchanges(&args(&["--store", &store_str, "amber1", "--summary"]));
    assert_eq!(sum_out.exit_code, 0);
    assert!(sum_out.stdout.contains("M a.txt"));
    assert!(sum_out.stdout.contains("A b.txt"));

    // --history
    let hist_out = run_mchanges(&args(&["--store", &store_str, "amber1", "--history"]));
    assert_eq!(hist_out.exit_code, 0);
    assert!(hist_out.stdout.contains("call=call-1"));

    // Default composed diff scoped to a.txt
    let diff_out = run_mchanges(&args(&["--store", &store_str, "amber1", "a.txt"]));
    assert_eq!(diff_out.exit_code, 0);
    assert!(diff_out.stdout.contains("-line1\n"));
    assert!(diff_out.stdout.contains("+line1-mod\n"));
    assert!(!diff_out.stdout.contains("b.txt"));

    // Write live workspace with an unrelated edit on line3 of a.txt.
    fs::write(&a_path, b"line1-mod\nline2\nline3-unrelated\n").expect("write a");
    fs::write(&b_path, b"brand-new\n").expect("write b");

    // Path-scoped revert of b.txt deletes b.txt and leaves a.txt untouched.
    let rev_b = run_mchanges(&args(&[
        "revert", "--store", &store_str, "--thread", "thread-1", "amber1", "b.txt",
    ]));
    assert_eq!(rev_b.exit_code, 0, "stderr: {}", rev_b.stderr);
    assert!(rev_b.stdout.contains("undo: mchanges revert amber2"));
    assert!(!b_path.exists());
    assert_eq!(
        fs::read_to_string(&a_path).expect("read a"),
        "line1-mod\nline2\nline3-unrelated\n"
    );

    // Reapply b.txt restores it.
    let app_b = run_mchanges(&args(&[
        "apply", "--store", &store_str, "--thread", "thread-1", "amber1", "b.txt",
    ]));
    assert_eq!(app_b.exit_code, 0, "stderr: {}", app_b.stderr);
    assert_eq!(fs::read_to_string(&b_path).expect("read b"), "brand-new\n");

    // Corrupt evidence must fail explicitly with exit code 1 rather than empty diff.
    let blob_hash = match &r1.files[0].after {
        FileState::Present { blob_hash, .. } => blob_hash.clone(),
        FileState::Missing => panic!("expected blob"),
    };
    fs::remove_file(store_dir.join("blobs").join(blob_hash)).expect("delete blob");
    let corrupt_out = run_mchanges(&args(&["--store", &store_str, "amber1"]));
    assert_eq!(corrupt_out.exit_code, 1);
    assert!(corrupt_out.stderr.contains("missing change evidence"));
}

#[test]
fn range_summary_omits_reverted_files_and_flags_external_write_gaps() {
    let dir = TempDir::new().expect("tempdir");
    let ws = dir.path().join("ws");
    let store_dir = dir.path().join("store");
    fs::create_dir_all(&ws).expect("ws");
    let store = ChangeHistoryStore::open(&store_dir).expect("open");

    let meta = |call: &str, kind: ChangeSourceKind| ChangeMetadata {
        thread_id: "thread-1".to_string(),
        session_id: "sess-1".to_string(),
        turn_id: format!("turn-{call}"),
        call_id: call.to_string(),
        source_kind: kind,
        workspace_root: ws.clone(),
        created_at: 1_700_000_000,
        exit_code: Some(0),
        summary: None,
        claim_key: None,
    };
    let file = |rel: &str, before: &[u8], after: &[u8]| PendingFileChange {
        rel_path: rel.to_string(),
        abs_path: ws.join(rel),
        moved_from: None,
        before: FileSnapshot::Present(before.to_vec()),
        after: FileSnapshot::Present(after.to_vec()),
        coverage: CoverageStatus::Complete,
    };

    store
        .record_change(
            meta("c1", ChangeSourceKind::ApplyPatch),
            vec![
                file("keep.rs", b"old\n", b"new\n"),
                file("restored.rs", b"orig\n", b"dirty\n"),
                file("gap.rs", b"g0\n", b"g1\n"),
            ],
        )
        .expect("amber1");
    store
        .record_change(
            meta("c2", ChangeSourceKind::VcsDiagnostic),
            vec![file("restored.rs", b"dirty\n", b"orig\n")],
        )
        .expect("amber2");
    store
        .record_change(
            meta("c3", ChangeSourceKind::ApplyPatch),
            vec![file("gap.rs", b"g1-external\n", b"g2\n")],
        )
        .expect("amber3");

    let store_str = store_dir.to_string_lossy().into_owned();
    let list_default = run_mchanges(&args(&["--store", &store_str, "--list"]));
    assert_eq!(list_default.exit_code, 0);
    assert!(!list_default.stdout.contains("amber2"));

    let list_history = run_mchanges(&args(&["--store", &store_str, "--list", "--history"]));
    assert_eq!(list_history.exit_code, 0);
    assert!(list_history.stdout.contains("amber2\tvcs_diagnostic"));

    let sum = run_mchanges(&args(&[
        "--store",
        &store_str,
        "amber1..amber3",
        "--summary",
    ]));
    assert_eq!(sum.exit_code, 0, "stderr: {}", sum.stderr);
    assert!(sum.stdout.contains("M keep.rs"));
    assert!(!sum.stdout.contains("restored.rs"));
    assert!(
        sum.stdout.contains("M gap.rs") && sum.stdout.contains("amber3: discontinuous baseline")
    );
    assert!(sum.stdout.contains("Total: 2 files"));
}
