package terminalview_test

import (
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func TestUISnapshotAgentFaintRetainsNormalColor(t *testing.T) {
	p := activityui.Painter{}
	b := activityui.Block{Kind: "progress", Body: "Waiting", WaitTargets: []presentation.WaitTarget{{Name: "/root/agent1"}}}
	rows := append(p.Block(b, 80), "plain after wait")
	uisnapshot.AssertTerminal(t, "testdata/snapshots/agent_faint.txt", rows, 80)
}
