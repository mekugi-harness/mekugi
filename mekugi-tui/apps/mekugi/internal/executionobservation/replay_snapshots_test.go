package executionobservation

import (
	"fmt"
	"math/rand/v2"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
)

func snapshotTestHistory() mekugiHistory {
	files := []changecapture.File{
		{Path: "/w/text", Kind: changecapture.Text, Content: "before\r\n<&>\n", Size: 14},
		{Path: "/w/binary", Kind: changecapture.Binary, Size: 17, Hash: "binary-hash"},
		{Path: "/w/link", Kind: changecapture.Symlink, Link: "../target"},
		{Path: "/w/missing", Kind: changecapture.Absent},
		{Path: "/w/incomplete", Error: "bounded capture"},
	}
	return mekugiHistory{
		ToolName: "exec", Script: "original input", CarrierPayload: "unchanged stock input",
		ResolvedBaseline: &resolvedStockBaseline{Root: "/w", Shell: "bash", Files: files,
			Omitted: []changecapture.Omission{{Path: "/w/omitted", Reason: "time bound"}}},
		ExecObservation: &changecapture.Observation{Files: files, Class: "scoped", WindowStart: time.Unix(1700000000, 0).UTC(),
			Listings: []changecapture.Listing{{Root: "/w", Entries: map[string]string{"text": "f"}}}},
		NativePatches: []changecapture.PatchObservation{{Input: "exact patch input", Files: []changecapture.PatchFile{{
			BeforePath: "/w/text", AfterPath: "/w/renamed", Before: "old\n", Exists: true,
			TargetBefore: "target\n", TargetExists: true, TargetError: "target diagnostic",
		}}}, {Input: "no files"}},
		ReviewFiles: []diffmodel.ReviewFile{{BeforePath: "/w/text", AfterPath: "/w/text", Diff: "-old\n+new\n"},
			{BeforePath: "/w/incomplete", Incomplete: "missing before"}},
		HostResults: []binding.ToolResult{{Tool: applyPatchToolName}},
	}
}

func snapshotTestBlobs(t *testing.T, store *mekugiReplayStore) []string {
	t.Helper()
	names, err := filepath.Glob(filepath.Join(store.directory, "snapshot-*.json.gz"))
	if err != nil {
		t.Fatal(err)
	}
	return names
}

func TestInlineEditStorageFailureDoesNotPublishCall(t *testing.T) {
	t.Parallel()
	store, err := openMekugiReplayStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	store.maxBytes = 1
	if err := store.put(t.Context(), "/w", map[string]mekugiHistory{"call": snapshotTestHistory()}); err == nil {
		t.Fatal("accepted storage overflow")
	}
	if _, found, err := store.lookup(t.Context(), "/w", "call"); err != nil || found {
		t.Fatalf("published an envelope before snapshots were durable: %v %v", found, err)
	}
}

func TestInlineEditEvidenceAdmissionIsAtomic(t *testing.T) {
	t.Parallel()
	store, err := openMekugiReplayStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	random := rand.New(rand.NewPCG(1, 2))
	var files []changecapture.File
	for i := range 4 {
		content := make([]byte, 4096)
		for j := range content {
			content[j] = byte('a' + random.IntN(26))
		}
		files = append(files, changecapture.File{Path: fmt.Sprintf("/w/%d", i), Kind: changecapture.Text, Content: string(content), Size: int64(len(content))})
	}
	store.maxBytes = 10 << 10
	err = store.put(t.Context(), "/w", map[string]mekugiHistory{"call": {ToolName: "exec", ExecObservation: &changecapture.Observation{Files: files}}})
	if err == nil {
		t.Fatal("accepted a record whose blobs exceed the quota")
	}
	if blobs := snapshotTestBlobs(t, store); len(blobs) != 0 {
		t.Fatalf("rejected record left blobs behind: %v", blobs)
	}
	store.maxBytes = 0
	if err := store.put(t.Context(), "/w", map[string]mekugiHistory{"call": {ToolName: "exec", ExecObservation: &changecapture.Observation{Files: files}}}); err != nil {
		t.Fatal(err)
	}
	if blobs := snapshotTestBlobs(t, store); len(blobs) != 0 {
		t.Fatalf("inline record produced snapshot blobs: %v", blobs)
	}
	got, found, err := store.lookup(t.Context(), "/w", "call")
	if err != nil || !found || !reflect.DeepEqual(got.ExecObservation.Files, files) {
		t.Fatalf("round trip found=%v err=%v", found, err)
	}
}
