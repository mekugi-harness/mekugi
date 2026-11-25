package launch

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandargs"

	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func TestUISnapshotDebugHandoffAfterCodexExit(t *testing.T) {
	fixtures, err := filepath.Abs("testdata/snapshots")
	if err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct {
		name, exit string
		overrides  bool
		relative   bool
	}{
		{name: "disconnect", exit: "0"},
		{name: "failure", exit: "23"},
		{name: "overrides", exit: "0", overrides: true},
		{name: "relative-overrides", exit: "0", overrides: true, relative: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			directory := filepath.Join(t.TempDir(), "debug space's $(not-a-command)")
			if err := os.Mkdir(directory, 0o700); err != nil {
				t.Fatal(err)
			}
			t.Setenv("XDG_CONFIG_HOME", t.TempDir())
			t.Setenv("TMPDIR", directory)
			t.Setenv("MEKUGI_AX_OUTPUT", "")
			t.Setenv("PATH", directory+string(os.PathListSeparator)+os.Getenv("PATH"))
			t.Setenv("MEKUGI_INVOCATION_FIXTURE_EXIT", test.exit)
			script := "#!/bin/sh\nMEKUGI_INVOCATION_FIXTURE=1; export MEKUGI_INVOCATION_FIXTURE\nexec " + commandargs.Quote(os.Args[0]) + " \"$@\"\n"
			if err := os.WriteFile(filepath.Join(directory, "mekugi-app-server"), []byte(script), 0o700); err != nil {
				t.Fatal(err)
			}
			output, err := os.CreateTemp(directory, "stderr-")
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = output.Close() })
			old := os.Stderr
			os.Stderr = output
			defer func() { os.Stderr = old }()
			args := []string{"--debug"}
			if test.overrides {
				capture, reads := filepath.Join(directory, "capture.jsonl"), filepath.Join(directory, "reads.jsonl")
				if test.relative {
					t.Chdir(directory)
					capture = "capture.jsonl"
				}
				args = append(args, "--capture-output", capture)
				t.Setenv("MEKUGI_AX_OUTPUT", reads)
			}
			code, err := runServerSession(t.Context(), args, nil, sessionapi.NewLive(), t.TempDir(), &threadExecution{})
			if err == nil || (test.exit == "0" && code != 1) || (test.exit == "23" && code != 23) {
				t.Fatalf("wrap exit: %d, %v", code, err)
			}
			data, err := os.ReadFile(output.Name())
			if err != nil {
				t.Fatal(err)
			}
			after := string(data)
			if strings.Contains(after, "codex-finished") || strings.Count(after, "Debug bundle saved to:") != 1 {
				t.Fatalf("native host diagnostics leaked or post-exit handoff was missing: %q", data)
			}
			lines := strings.Split(strings.TrimSpace(after), "\n")
			if len(lines) != 1 {
				t.Fatalf("diagnostic handoff is not one line: %q", after)
			}
			dir := strings.TrimSpace(strings.TrimPrefix(lines[0], "Debug bundle saved to:"))
			if dir == "" {
				t.Fatalf("diagnostic handoff missing bundle directory: %q", after)
			}
			normalized := strings.ReplaceAll(after, dir, "/tmp/mekugi-debug-session")
			uisnapshot.Assert(t, filepath.Join(fixtures, "debug-handoff.txt"), normalized)
		})
	}
}

// Exercise the real handoff tests in a fresh package invocation with ambient
// retained state. Neither generated bundles nor retention may touch that state.
func TestDebugHandoffTestsIsolateAmbientState(t *testing.T) {
	state := t.TempDir()
	root := filepath.Join(state, "mekugi", "debug")
	oldBundle := filepath.Join(root, "mekugi-debug-existing")
	if err := os.MkdirAll(oldBundle, 0700); err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(oldBundle, ".mekugi-debug-v1.lock")
	if err := os.WriteFile(marker, nil, 0600); err != nil {
		t.Fatal(err)
	}
	old := time.Now().Add(-15 * 24 * time.Hour)
	if err := os.Chtimes(marker, old, old); err != nil {
		t.Fatal(err)
	}
	command := exec.CommandContext(t.Context(), os.Args[0], "-test.run=^TestUISnapshotDebugHandoffAfterCodexExit$")
	command.Env = append(os.Environ(), "XDG_STATE_HOME="+state, "MEKUGI_LAUNCHER_TEST_STATE_ISOLATED=")
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("handoff subprocess: %v\n%s", err, output)
	}
	entries, err := os.ReadDir(root)
	if err != nil || len(entries) != 1 || entries[0].Name() != filepath.Base(oldBundle) {
		t.Fatalf("ambient debug state changed: %v, %v", entries, err)
	}
	info, err := os.Stat(marker)
	if err != nil || !info.ModTime().Equal(old) {
		t.Fatalf("ambient retained bundle changed: %v, %v", info, err)
	}
}
