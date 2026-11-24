package sessionreview

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/activitydetail"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

type nativeStoreFixture struct {
	t     *testing.T
	dir   string
	index nativeStoreIndex
}

func newNativeStoreFixture(t *testing.T) *nativeStoreFixture {
	t.Helper()
	dir := t.TempDir()
	for _, sub := range []string{"blobs", "records"} {
		if err := os.MkdirAll(filepath.Join(dir, sub), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "store.lock"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	return &nativeStoreFixture{
		t:   t,
		dir: dir,
		index: nativeStoreIndex{
			SchemaVersion: 1,
			NextSequence:  1,
			ThreadStreams: map[string]string{},
		},
	}
}

func (f *nativeStoreFixture) blob(content []byte, binary bool) nativeFileState {
	f.t.Helper()
	sum := sha256.Sum256(content)
	hash := hex.EncodeToString(sum[:])
	if err := os.WriteFile(filepath.Join(f.dir, "blobs", hash), content, 0o600); err != nil {
		f.t.Fatal(err)
	}
	return nativeFileState{
		State:    "present",
		BlobHash: hash,
		ByteLen:  int64(len(content)),
		IsBinary: binary,
	}
}

func (f *nativeStoreFixture) publish(stream string, record nativeChangeRecord) {
	f.t.Helper()
	record.SchemaVersion = 1
	if record.Sequence == 0 {
		record.Sequence = f.index.NextSequence
		f.index.NextSequence++
	}
	f.index.ThreadStreams[record.Metadata.ThreadID] = stream
	recordBytes, err := json.Marshal(record)
	if err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.dir, "records", record.ID+".json"), recordBytes, 0o600); err != nil {
		f.t.Fatal(err)
	}
	var paths []string
	incomplete := false
	for _, file := range record.Files {
		paths = append(paths, file.RelPath)
		if file.Coverage.Status != "complete" {
			incomplete = true
		}
	}
	f.index.Entries = append(f.index.Entries, nativeIndexEntry{
		ID:                    record.ID,
		Sequence:              record.Sequence,
		ThreadID:              record.Metadata.ThreadID,
		CallID:                record.Metadata.CallID,
		SourceKind:            record.Metadata.SourceKind,
		CreatedAt:             record.Metadata.CreatedAt,
		FilePaths:             paths,
		HasIncompleteCoverage: incomplete,
	})
	indexBytes, err := json.Marshal(f.index)
	if err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.dir, "index.json"), indexBytes, 0o600); err != nil {
		f.t.Fatal(err)
	}
}

func TestNativeStoreHydratesAndRefreshesSavedDiff(t *testing.T) {
	workspace := t.TempDir()
	f := newNativeStoreFixture(t)

	f.publish("a", nativeChangeRecord{
		ID: "amber1",
		Metadata: nativeChangeMetadata{
			ThreadID:      "main",
			SessionID:     "sess-1",
			CallID:        "call-1",
			SourceKind:    "apply_patch",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{
			{
				RelPath:  "hello.txt",
				AbsPath:  filepath.Join(workspace, "hello.txt"),
				Kind:     nativeFileChangeKind{Kind: "add"},
				Before:   nativeFileState{State: "missing"},
				After:    f.blob([]byte("hello\nworld\n"), false),
				Coverage: nativeCoverageStatus{Status: "complete"},
			},
		},
	})
	f.publish("a", nativeChangeRecord{
		ID: "amber2",
		Metadata: nativeChangeMetadata{
			ThreadID:      "main",
			SessionID:     "sess-1",
			CallID:        "call-git",
			SourceKind:    "vcs_diagnostic",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{
			{
				RelPath:  "git-only.txt",
				AbsPath:  filepath.Join(workspace, "git-only.txt"),
				Kind:     nativeFileChangeKind{Kind: "add"},
				Before:   nativeFileState{State: "missing"},
				After:    f.blob([]byte("diagnostic\n"), false),
				Coverage: nativeCoverageStatus{Status: "complete"},
			},
		},
	})
	f.publish("b", nativeChangeRecord{
		ID: "apple1",
		Metadata: nativeChangeMetadata{
			ThreadID:      "other-thread",
			SessionID:     "sess-1",
			CallID:        "call-other",
			SourceKind:    "apply_patch",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{
			{
				RelPath:  "other.txt",
				AbsPath:  filepath.Join(workspace, "other.txt"),
				Kind:     nativeFileChangeKind{Kind: "add"},
				Before:   nativeFileState{State: "missing"},
				After:    f.blob([]byte("outside scope\n"), false),
				Coverage: nativeCoverageStatus{Status: "complete"},
			},
		},
	})

	review := New(t.Context(), nil, nil)
	defer review.Close()
	review.SetNativeDir(f.dir)
	review.SetCaller("main", "/root")
	review.IncludeThread(workspace, "main", true)
	review.Projection.NativeDir = f.dir
	review.Loader = NewLoader(t.Context(), review.Projection)
	if review.Loader == nil {
		t.Fatal("expected Loader to initialize when NativeDir is set")
	}
	review.Loader.AwaitResync()
	sub := review.Subscribe()
	defer review.Unsubscribe(sub)

	drainLoader := func() {
		t.Helper()
		for {
			select {
			case event := <-sub.Events():
				review.Apply(t.Context(), event)
			default:
				if !review.Loader.Pending() {
					return
				}
				select {
				case result := <-review.Loader.Updates():
					review.Loaded(result)
				case <-time.After(5 * time.Second):
					t.Fatal("timed out waiting for native review loader")
				}
			}
		}
	}

	drainLoader()
	if review.Failure != "" {
		t.Fatalf("unexpected review failure: %s", review.Failure)
	}
	pub := review.Projection.Data.Publication
	if len(pub.Files) != 1 || filepath.Base(pub.Files[0].Path) != "hello.txt" {
		t.Fatalf("expected 1 authored file (hello.txt), got %d: %+v", len(pub.Files), pub.Files)
	}
	if len(pub.Diagnostics) != 1 {
		t.Fatalf("expected 1 diagnostic VCS chunk, got %d", len(pub.Diagnostics))
	}
	if pub.Counts.Net == nil || pub.Counts.Net.Added != 2 || pub.Counts.Net.Removed != 0 {
		t.Fatalf("unexpected net counts: %+v", pub.Counts.Net)
	}

	// Publish a new native edit from an admitted child thread and refresh incrementally.
	review.SetCaller("child", "/root/worker")
	review.Observe(workspace, "child", "main", false)
	f.publish("c", nativeChangeRecord{
		ID: "arch1",
		Metadata: nativeChangeMetadata{
			ThreadID:      "child",
			SessionID:     "sess-1",
			CallID:        "call-child-1",
			SourceKind:    "shell_write",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{
			{
				RelPath:  "hello.txt",
				AbsPath:  filepath.Join(workspace, "hello.txt"),
				Kind:     nativeFileChangeKind{Kind: "modify"},
				Before:   f.blob([]byte("hello\nworld\n"), false),
				After:    f.blob([]byte("hello\nworld\nfrom child\n"), false),
				Coverage: nativeCoverageStatus{Status: "complete"},
			},
		},
	})
	review.RefreshNative()
	drainLoader()

	if review.Failure != "" {
		t.Fatalf("unexpected failure after incremental refresh: %s", review.Failure)
	}
	pub = review.Projection.Data.Publication
	if pub.Counts.Net == nil || pub.Counts.Net.Added != 3 || pub.Counts.Net.Removed != 0 {
		t.Fatalf("expected 3 net added lines after child edit, got %+v", pub.Counts.Net)
	}
	if _, ok := pub.Counts.Callers["/root/worker"]; !ok {
		t.Fatalf("expected /root/worker caller attribution, got %+v", pub.Counts.Callers)
	}

	// Also verify binary and move records hydrate cleanly.
	f.publish("a", nativeChangeRecord{
		ID: "amber3",
		Metadata: nativeChangeMetadata{
			ThreadID:      "main",
			SessionID:     "sess-1",
			CallID:        "call-bin",
			SourceKind:    "apply_patch",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{{
			RelPath:  "img.bin",
			AbsPath:  filepath.Join(workspace, "img.bin"),
			Kind:     nativeFileChangeKind{Kind: "add"},
			Before:   nativeFileState{State: "missing"},
			After:    f.blob([]byte{0x00, 0x01, 0x02}, true),
			Coverage: nativeCoverageStatus{Status: "complete"},
		}},
	})
	review.RefreshNative()
	drainLoader()
	if review.Failure != "" {
		t.Fatalf("unexpected failure after binary refresh: %s", review.Failure)
	}
	if len(review.Projection.Data.Publication.Files) != 2 {
		t.Fatalf("expected 2 authored files after binary capture, got %d", len(review.Projection.Data.Publication.Files))
	}
}

func TestNativeStoreDeduplicatesLegacyUserShellCall(t *testing.T) {
	workspace := t.TempDir()
	f := newNativeStoreFixture(t)
	f.publish("a", nativeChangeRecord{
		ID: "amber1",
		Metadata: nativeChangeMetadata{
			ThreadID:      "main",
			SessionID:     "sess-dedup",
			CallID:        "shell-call-1",
			SourceKind:    "shell_write",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{
			{
				RelPath:  "note.txt",
				AbsPath:  filepath.Join(workspace, "note.txt"),
				Kind:     nativeFileChangeKind{Kind: "add"},
				Before:   nativeFileState{State: "missing"},
				After:    f.blob([]byte("from shell\n"), false),
				Coverage: nativeCoverageStatus{Status: "complete"},
			},
		},
	})

	builder := presentation.NewBuilder(nil)
	if err := builder.Add(presentation.Attempt{
		AttemptIdentity: presentation.AttemptIdentity{
			Key:    presentation.AttemptKey{Workspace: workspace, Call: "legacy-shell-1"},
			Change: "amber1",
		},
		Operation: activitydetail.Operation{Thread: "main", Item: "shell-call-1"},
		Chunks: []presentation.Chunk{{
			Key:          workspace + "\x00legacy-shell-1/0",
			Stream:       workspace + "\x00legacy\x000",
			Workspace:    workspace,
			CaptureOrder: 1,
			Review:       diffmodel.RenderReviewFile("", filepath.Join(workspace, "note.txt"), "", "from shell\n"),
			Origin:       presentation.Origin{Change: "amber1", Caller: "/root", Source: "userShell"},
		}},
	}); err != nil {
		t.Fatal(err)
	}
	base := builder.Finish()

	proj := Projection{
		NativeDir: f.dir,
		Data:      base,
		Scope:     previewmodel.Scope{Workspaces: map[string]map[string]bool{workspace: {"main": true}}},
	}
	changed, err := proj.Apply(t.Context(), Event{Kind: "native", NativeDir: f.dir})
	if err != nil {
		t.Fatalf("Apply(native): %v", err)
	}
	if changed {
		t.Fatal("duplicate native record for legacy userShell call was not skipped")
	}
	if proj.Data.Len() != 1 {
		t.Fatalf("expected 1 attempt after dedup, got %d", proj.Data.Len())
	}
}

func TestNativeStoreRejectsInvalidIDAndCorruptBlob(t *testing.T) {
	workspace := t.TempDir()
	f := newNativeStoreFixture(t)
	f.index.Entries = append(f.index.Entries, nativeIndexEntry{ID: "../escape", ThreadID: "main"})
	indexBytes, _ := json.Marshal(f.index)
	if err := os.WriteFile(filepath.Join(f.dir, "index.json"), indexBytes, 0o600); err != nil {
		t.Fatal(err)
	}
	proj := Projection{NativeDir: f.dir, Data: new(presentation.Snapshot)}
	scope := previewmodel.Scope{Workspaces: map[string]map[string]bool{workspace: {"main": true}}}
	if _, err := proj.Apply(t.Context(), Event{Kind: "scope", Scope: &scope, NativeDir: f.dir, Resync: true}); err == nil || !strings.Contains(err.Error(), "invalid change ID") {
		t.Fatalf("expected invalid change ID error, got %v", err)
	}

	f2 := newNativeStoreFixture(t)
	state := f2.blob([]byte("good\n"), false)
	if err := os.WriteFile(filepath.Join(f2.dir, "blobs", state.BlobHash), []byte("evil\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	f2.publish("a", nativeChangeRecord{
		ID: "amber1",
		Metadata: nativeChangeMetadata{
			ThreadID:      "main",
			SourceKind:    "apply_patch",
			WorkspaceRoot: workspace,
		},
		Files: []nativeFileEvidence{{
			RelPath:  "a.txt",
			AbsPath:  filepath.Join(workspace, "a.txt"),
			Kind:     nativeFileChangeKind{Kind: "add"},
			Before:   nativeFileState{State: "missing"},
			After:    state,
			Coverage: nativeCoverageStatus{Status: "complete"},
		}},
	})
	proj2 := Projection{NativeDir: f2.dir, Data: new(presentation.Snapshot)}
	if _, err := proj2.Apply(t.Context(), Event{Kind: "scope", Scope: &scope, NativeDir: f2.dir, Resync: true}); err == nil || !strings.Contains(err.Error(), "checksum mismatch") {
		t.Fatalf("expected checksum mismatch error, got %v", err)
	}
}
