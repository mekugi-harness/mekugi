package executionobservation

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	changepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

func TestNativeExecSedInPlaceCapturesEdit(t *testing.T) {
	proxy := newManagedMekugiProxy(t)
	attachTestReplayStore(t, proxy)
	workspace := t.TempDir()
	const relativePath = "internal/router/live_activity_view_test.go"
	const before = "package router\n\nvar answer = response.Answers[0].Id\n"
	const after = "package router\n\nvar answer = response.Answers[0].ID\n"
	command := sedInPlace() + ` 's/Answers\[0\]\.Id/Answers[0].ID/' internal/router/live_activity_view_test.go`
	target := filepath.Join(workspace, relativePath)
	writeTestFile(t, target, before)
	history, ctx := runSnapshotExec(t, proxy, workspace, "sed-call", command, func(context.Context) {
		cmd := exec.CommandContext(t.Context(), execTrackShellExecutable(t, "bash"), "--noprofile", "--norc", "-c", command)
		cmd.Dir = workspace
		cmd.Env = append(os.Environ(), "BASH_ENV=")
		if output, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("sed: %v\n%s", err, output)
		}
		if content, err := os.ReadFile(target); err != nil || string(content) != after {
			t.Fatalf("edited content = %q, err = %v", content, err)
		}
	})
	if history.ChangeID == "" || len(history.ReviewFiles) != 1 {
		t.Fatalf("persisted sed edit = %+v", history)
	}
	scoped := testProxyStore(proxy).scoped(ctx)
	changes, err := scoped.changes.Read(ctx, scoped.changeScope(), changes.Query{
		Workspace: workspace, IDs: []string{history.ChangeID}, View: "history",
	})
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{relativePath, "-var answer = response.Answers[0].Id", "+var answer = response.Answers[0].ID", "exec_command input:\n" + command} {
		if !strings.Contains(changes, want) {
			t.Errorf("mchanges history missing %q:\n%s", want, changes)
		}
	}
	receipt := changepresentation.ReceiptText(workspace, receiptEvidence(history))
	if !strings.HasPrefix(receipt, "Edit `"+relativePath+"`") || !strings.Contains(receipt, " · sed") {
		t.Fatalf("sed receipt should identify an Edit and sed label: %q", receipt)
	}
}
