use std::path::Path;

use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeScope;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use pretty_assertions::assert_eq;

use super::CAPTURE_PAGE_BYTES;
use super::CAPTURE_PATCH_BYTES;
use super::ChangeHistoryStore;
use super::MAX_RESPONSE_BYTES;
use super::ThreadChangesReadParams;
use super::parse_cursor;
use super::read_changes;

fn record(store: &ChangeHistoryStore, workspace: &Path, name: &str, files: Vec<(&str, String)>) {
    store
        .record_change(
            ChangeMetadata {
                thread_id: "thread".into(),
                session_id: "session".into(),
                turn_id: "turn".into(),
                call_id: name.into(),
                author: None,
                source_kind: ChangeSourceKind::ShellWrite,
                workspace_root: workspace.to_path_buf(),
                created_at: 0,
                exit_code: Some(0),
                summary: None,
                claim_key: None,
            },
            files
                .into_iter()
                .map(|(path, text)| PendingFileChange {
                    rel_path: path.into(),
                    abs_path: workspace.join(path),
                    moved_from: None,
                    before: FileSnapshot::Missing,
                    after: FileSnapshot::Present(text.into_bytes()),
                    coverage: CoverageStatus::Complete,
                    scope: ChangeScope::Workspace,
                })
                .collect(),
        )
        .expect("record captured files");
}

#[test]
fn capture_pages_bound_encoded_payload_and_resume_inside_a_patch() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let dir = workspace.path().join("history");
    let store = ChangeHistoryStore::open(dir.clone()).expect("open history");
    // Escape-heavy text measures encoded bytes, not just unescaped patch size.
    let escaped = "\u{1}".repeat(380_000);
    let large = "界".repeat(CAPTURE_PATCH_BYTES / 2);
    record(
        &store,
        workspace.path(),
        "first",
        vec![
            ("a", escaped.clone()),
            ("b", escaped),
            ("c", large),
            ("d", "final\n".into()),
        ],
    );
    let mut params = ThreadChangesReadParams {
        thread_id: "thread".into(),
        include_ephemeral: None,
        caller_thread_id: None,
        item_id: None,
        per_capture: true,
        limit: None,
        cursor: None,
    };
    let expected =
        read_changes(dir.clone(), None, None, params.clone(), None).expect("unpaged read");
    params.limit = Some(100);
    let mut cursor = None;
    let mut actual: Vec<(String, String)> = Vec::new();
    for page_index in 0..8 {
        let page =
            read_changes(dir.clone(), None, None, params.clone(), cursor).expect("capture page");
        assert!(serde_json::to_vec(&page).expect("encode page").len() < CAPTURE_PAGE_BYTES);
        if page_index == 0 {
            assert_eq!(page.files.len(), 1, "encoded-byte budget ended first page");
            record(
                &store,
                workspace.path(),
                "later",
                vec![("e", "new capture\n".into())],
            );
        }
        for file in page.files {
            if let Some(last) = actual.last_mut().filter(|last| last.0 == file.path) {
                last.1.push_str(&file.unified_diff);
            } else {
                actual.push((file.path, file.unified_diff));
            }
        }
        let Some(next) = page.next_cursor else {
            assert_eq!(
                actual,
                expected
                    .files
                    .into_iter()
                    .map(|file| (file.path, file.unified_diff))
                    .collect::<Vec<_>>()
            );
            return;
        };
        cursor = Some(parse_cursor(&next).expect("valid continuation cursor"));
    }
    panic!("capture pagination never completed");
}

#[test]
fn composed_review_pages_keep_totals_and_provenance_under_the_connector_limit() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let dir = workspace.path().join("history");
    let store = ChangeHistoryStore::open(dir.clone()).expect("open history");
    let escaped = "\u{1}".repeat(6 << 20);
    record(
        &store,
        workspace.path(),
        "command",
        vec![
            ("a", escaped.clone()),
            ("b", escaped),
            ("too-large", "\u{1}".repeat(11 << 20)),
            ("z", "final\n".into()),
        ],
    );
    for caller in [None, Some("thread".to_string())] {
        let params = ThreadChangesReadParams {
            thread_id: "thread".into(),
            include_ephemeral: None,
            caller_thread_id: caller,
            item_id: None,
            per_capture: false,
            limit: Some(20),
            cursor: None,
        };
        let mut cursor = None;
        for (index, expected_paths) in [vec!["a"], vec!["b"], vec!["too-large", "z"]]
            .into_iter()
            .enumerate()
        {
            let page =
                read_changes(dir.clone(), None, None, params.clone(), cursor).expect("review page");
            assert!(serde_json::to_vec(&page).expect("encode page").len() < MAX_RESPONSE_BYTES);
            assert_eq!(page.file_count, 4);
            assert_eq!((page.total_additions, page.total_deletions), (4, 0));
            assert_eq!(
                page.files
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>(),
                expected_paths
            );
            assert_eq!(page.changes.len(), 1);
            assert!(
                page.files
                    .iter()
                    .all(|file| file.change_ids == [page.changes[0].change_id.as_str()])
            );
            if index < 2 {
                assert!(page.files[0].coverage_notes.is_empty());
                assert!(page.files[0].unified_diff.contains('\u{1}'));
                let next = page.next_cursor.expect("continued review");
                cursor = Some(parse_cursor(&next).expect("valid review continuation"));
            } else {
                assert_eq!(
                    page.files[0].coverage_notes,
                    ["Patch exceeds the Saved Diff display limit."]
                );
                assert!(page.files[0].unified_diff.is_empty());
                assert!(page.files[1].unified_diff.contains("+final"));
                assert_eq!(page.next_cursor, None);
            }
        }
    }
}
