package launch

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
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
