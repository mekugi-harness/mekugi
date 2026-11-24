package launch

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

func writeNativeCapture(t *testing.T, sessionID, workspace, thread, stream, id, callID, relPath, before, after string, seq uint64) {
	t.Helper()
	dir := sessionreview.NativeStoreDir(sessionID)
	for _, sub := range []string{"blobs", "records"} {
		if err := os.MkdirAll(filepath.Join(dir, sub), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	writeBlob := func(text string) map[string]any {
		if text == "" {
			return map[string]any{"state": "missing"}
		}
		sum := sha256.Sum256([]byte(text))
		hash := hex.EncodeToString(sum[:])
		if err := os.WriteFile(filepath.Join(dir, "blobs", hash), []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
		return map[string]any{"state": "present", "blob_hash": hash, "byte_len": len(text), "is_binary": false}
	}
	kind := "modify"
	if before == "" {
		kind = "add"
	}
	record := map[string]any{
		"schema_version": 1,
		"id":             id,
		"sequence":       seq,
		"metadata": map[string]any{
			"thread_id":      thread,
			"session_id":     sessionID,
			"turn_id":        "turn-1",
			"call_id":        callID,
			"source_kind":    "apply_patch",
			"workspace_root": workspace,
			"created_at":     1700000000,
		},
		"files": []map[string]any{{
			"rel_path": relPath,
			"abs_path": filepath.Join(workspace, relPath),
			"kind":     map[string]any{"kind": kind},
			"before":   writeBlob(before),
			"after":    writeBlob(after),
			"coverage": map[string]any{"status": "complete"},
		}},
	}
	recordBytes, err := json.Marshal(record)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "records", id+".json"), recordBytes, 0o600); err != nil {
		t.Fatal(err)
	}
	var index struct {
		SchemaVersion uint32            `json:"schema_version"`
		NextSequence  uint64            `json:"next_sequence"`
		ThreadStreams map[string]string `json:"thread_streams"`
		Entries       []map[string]any  `json:"entries"`
	}
	if existing, err := os.ReadFile(filepath.Join(dir, "index.json")); err == nil {
		if err := json.Unmarshal(existing, &index); err != nil {
			t.Fatal(err)
		}
	} else {
		index.SchemaVersion = 1
		index.ThreadStreams = map[string]string{}
	}
	index.NextSequence = seq + 1
	index.ThreadStreams[thread] = stream
	index.Entries = append(index.Entries, map[string]any{
		"id":                      id,
		"sequence":                seq,
		"thread_id":               thread,
		"call_id":                 callID,
		"source_kind":             "apply_patch",
		"created_at":              1700000000,
		"file_paths":              []string{relPath},
		"has_incomplete_coverage": false,
	})
	indexBytes, err := json.Marshal(index)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "index.json"), indexBytes, 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestSavedDiffIncludesNativeCapturedEdits(t *testing.T) {
	t.Setenv("TMPDIR", t.TempDir())
	workspace := savedDiffWorkspace(t)
	sessionID := "native-saved-diff-session"

	u := newAppServerSessionTestUI(t, workspace)
	var panel *appServerStatusReport
	var resumePicker *appServerResumePicker
	sub := u.review.Subscribe()
	defer u.review.Unsubscribe(sub)
	apply := func() {
		t.Helper()
		for _, event := range u.review.TakePreviews(sub) {
			applyTestReview(&u.attached, u.shell, u.review, u.reviewSource, u.preparePane, &u.pendingEditReceipts, u.clock, t.Context(), event)
		}
		if u.review.Failure != "" {
			t.Fatal(u.review.Failure)
		}
	}
	paths := func() []string {
		var result []string
		for _, file := range u.review.Projection.Data.Publication.Files {
			result = append(result, filepath.Base(file.Path))
		}
		slices.Sort(result)
		return result
	}

	restoreTestDiffThread(&u.attached, u.shell, u.review, u.reviewSource, u.preparePane, binding.ThreadInfo{
		ID:        "main",
		SessionID: sessionID,
		Cwd:       workspace,
	})
	apply()
	if got := paths(); len(got) != 0 {
		t.Fatalf("expected empty Saved Diff before edits, got %v", got)
	}

	// Simulate a newly native-captured apply_patch edit followed by item/completed.
	writeNativeCapture(t, sessionID, workspace, "main", "a", "amber1", "patch-call-1", "main.go", "", "package main\n", 1)
	if err := u.observedEvent(&u.attached, &u.picker, &u.Editor, u.clock, &panel, &resumePicker, "item/completed", binding.Notification{
		ThreadID: "main",
		TurnID:   "turn-1",
		Item:     binding.ThreadItem{ID: "patch-call-1", Type: "fileChange", Status: "completed"},
	}); err != nil {
		t.Fatal(err)
	}
	apply()
	if got := paths(); !slices.Equal(got, []string{"main.go"}) {
		t.Fatalf("expected native edit in Saved Diff, got %v", got)
	}

	// Simulate a child thread starting and writing a native capture.
	if err := u.observedEvent(&u.attached, &u.picker, &u.Editor, u.clock, &panel, &resumePicker, "thread/started", binding.Notification{
		Thread: binding.ThreadInfo{ID: "child-1", SessionID: sessionID, ParentThreadID: "main", Cwd: workspace},
	}); err != nil {
		t.Fatal(err)
	}
	writeNativeCapture(t, sessionID, workspace, "child-1", "b", "apple1", "child-call-1", "child.go", "", "package child\n", 2)
	if err := u.observedEvent(&u.attached, &u.picker, &u.Editor, u.clock, &panel, &resumePicker, "turn/completed", binding.Notification{
		ThreadID: "child-1",
		Turn:     binding.Turn{ID: "turn-child-1", Status: "completed"},
	}); err != nil {
		t.Fatal(err)
	}
	apply()
	if got := paths(); !slices.Equal(got, []string{"child.go", "main.go"}) {
		t.Fatalf("expected main + child native edits in Saved Diff, got %v", got)
	}
}
