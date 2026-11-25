package sessionreview

import (
	"slices"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func TestCapturedReceiptUsesHostIdentity(t *testing.T) {
	for _, test := range []struct {
		name     string
		codeMode bool
		results  []history.HostResult
		want     []string
	}{
		{"native shell", false, []history.HostResult{{Tool: "userShell", CallID: "host-shell"}}, []string{"host-shell"}},
		{"code mode", true, []history.HostResult{{Tool: "exec_command", CallID: "first"}, {Tool: "exec_command", CallID: "second"}}, []string{"first", "second"}},
		{"legacy", false, nil, []string{"retained"}},
	} {
		t.Run(test.name, func(t *testing.T) {
			receipt := CapturedReceipt(t.TempDir(), history.History{
				ExecutingThread: "thread", CorrelationID: "retained\x00exec",
				ExecOutcome: &history.Outcome{CodeMode: test.codeMode}, HostResults: test.results,
			}, nil)
			if receipt == nil || !slices.Equal(receipt.Calls, test.want) {
				t.Fatalf("receipt = %+v, want calls %v", receipt, test.want)
			}
		})
	}
}

func TestCapturedPatchReceiptRetainsWindowHostIdentities(t *testing.T) {
	receipt := CapturedReceipt(t.TempDir(), history.History{ToolName: "apply_patch", ExecutingThread: "thread", HostResults: []history.HostResult{{Tool: "apply_patch", CallID: "delete"}, {Tool: "apply_patch", CallID: "recreate"}}}, nil)
	if receipt == nil || receipt.Thread != "thread" || !slices.Equal(receipt.Calls, []string{"delete", "recreate"}) {
		t.Fatalf("restored patch window lost host identities: %+v", receipt)
	}
}
