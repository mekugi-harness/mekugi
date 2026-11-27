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
