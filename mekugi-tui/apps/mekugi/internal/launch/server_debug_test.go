package launch

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

func TestDebugSessionArtifacts(t *testing.T) {
	t.Setenv("TMPDIR", shortTempDir(t))
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	var paths []string
	err := testRunSession(testHistory(), ctx, testCommands(), []string{"--debug"}, nil, func(session invocationFixture) {
		if session.AXReadOutput == "" {
			t.Error("debug did not enable runtime AX reads")
		}
		cancel()
	}, func(artifacts []string) { paths = artifacts })
	if err != nil || len(paths) != 6 {
		t.Fatalf("debug session: %v, paths %v", err, paths)
	}
	for _, path := range paths {
		info, err := os.Stat(path)
		if err != nil || !filepath.IsAbs(path) || info.Mode().Perm() != 0o600 {
			t.Fatalf("artifact %s: %v, %v", path, info, err)
		}
	}
	log, _ := os.ReadFile(paths[0])
	if !bytes.Contains(log, []byte("router_start")) || !bytes.Contains(log, []byte("router_stop")) {
		t.Fatal("missing lifecycle diagnostics")
	}
	metrics, _ := os.ReadFile(paths[2])
	var exported struct {
		StorageWrites *struct {
			Bytes uint64 `json:"bytes"`
			Scope string `json:"scope"`
		} `json:"storage_writes"`
	}
	if err := json.Unmarshal(metrics, &exported); err != nil || exported.StorageWrites == nil || exported.StorageWrites.Bytes == 0 || exported.StorageWrites.Scope == "" {
		t.Fatalf("missing application write accounting: %s, %v", metrics, err)
	}
	root, err := debugStorageDirectory()
	if err != nil || filepath.Dir(filepath.Dir(paths[0])) != root {
		t.Fatalf("bundle not in managed state: %v, %v", paths, err)
	}
}

func TestDebugCustomCaptureBundleSupportsReplay(t *testing.T) {
	t.Setenv("TMPDIR", shortTempDir(t))
	custom := filepath.Join(t.TempDir(), "custom capture.jsonl")
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	var paths []string
	if err := testRunSession(testHistory(), ctx, testCommands(), []string{"--debug", "--capture-output", custom}, nil,
		func(invocationFixture) { cancel() }, func(artifacts []string) { paths = artifacts }); err != nil {
		t.Fatal(err)
	}
	if len(paths) != 6 || paths[1] != custom {
		t.Fatalf("selected capture destination changed: %q", paths)
	}
	debugDir := filepath.Dir(paths[0])
	bundle, err := os.Stat(filepath.Join(debugDir, "capture.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	selected, err := os.Stat(custom)
	if err != nil || !os.SameFile(bundle, selected) {
		t.Fatalf("debug capture does not resolve to the selected file: %v", err)
	}
	// Representative retained provider evidence in the real producer's chosen
	// capture destination must reach the existing replay consumer.
	replayTestWrite(t, filepath.Dir(custom), filepath.Base(custom), map[string]any{
		"boundary": "provider", "thread_id": "root", "captured_at": time.UnixMilli(replayTestEpoch + 50), "duration_ms": 20,
	})
	rollout := replayTestWrite(t, t.TempDir(), "rollout-root.jsonl", replayTestMeta("root"),
		replayTestRecord("event_msg", replayTestEpoch, map[string]any{"type": "task_started", "turn_id": "turn"}),
		replayTestItem("root", "turn", replayTestEpoch+1, replayTestEpoch+2, map[string]any{
			"type": "AgentMessage", "id": "answer", "content": []map[string]any{{"text": "Replay complete."}},
		}))
	replay, err := replayReadTest(t, new(transportcapture.Service), t.Context(), rollout, debugDir, 1)
	if err != nil || replay == nil || replay.Providers != 1 {
		t.Fatalf("custom capture replay: %v, error=%v", replay, err)
	}
}

func TestDebugStartupFailureReportsArtifacts(t *testing.T) {
	t.Setenv("TMPDIR", shortTempDir(t))
	missing := filepath.Join(t.TempDir(), "missing", "capture.jsonl")
	var paths []string
	err := testRunSession(testHistory(), t.Context(), testCommands(), []string{"--debug", "--capture-output", missing}, nil, func(invocationFixture) {
		t.Error("ready despite capture initialization failure")
	}, func(artifacts []string) { paths = artifacts })
	if err == nil || len(paths) != 6 {
		t.Fatalf("startup failure lost artifact paths: %v", err)
	}
}

func TestDebugCanceledStartupReportsArtifacts(t *testing.T) {
	t.Setenv("TMPDIR", shortTempDir(t))
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	var paths []string
	err := testRunSession(testHistory(), ctx, testCommands(), []string{"--debug"}, nil, func(invocationFixture) {
		t.Error("canceled startup reached readiness")
	}, func(artifacts []string) { paths = artifacts })
	if err != nil || len(paths) != 6 {
		t.Fatalf("canceled startup lost paths: %v, %v", paths, err)
	}
}
