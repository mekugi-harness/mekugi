package executionobservation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis"

	"testing"

	activitymodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/segments"
	segmenttransport "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/transport"
)

func TestExecWindowRepeatedOpenKeepsOneReportOwner(t *testing.T) {
	t.Parallel()
	server, err := segmenttransport.Listen(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(server.Close)
	observer, err := segments.NewObserver(server, activitymodel.OutputBytes)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(observer.Close)
	registry := &Windows{Reports: observer}
	command := commandanalysis.Command{Command: "mktemp", Workdir: t.TempDir(), Shell: "bash"}
	first := &execWindow{ref: "call", thread: "thread", turn: "turn"}
	registry.open(first, command)
	registry.open(&execWindow{ref: "call", thread: "thread", turn: "turn"}, command)
	if first.tracking == nil {
		t.Fatal("missing first subscription")
	}
	state := first.tracking.State()
	if len(state.Matches) != 1 || state.Matches[0].Ambiguous {
		t.Fatalf("repeated open changed report ownership: %+v", state)
	}
	child := registry.ForRuntime()
	childWindow := &execWindow{ref: "child", thread: "thread", turn: "turn"}
	child.open(childWindow, command)
	if childWindow.tracking != nil || child.Reports != nil {
		t.Fatal("child inherited another runtime's report subscription")
	}
	if state := first.tracking.State(); len(state.Matches) != 1 || state.Matches[0].Ambiguous {
		t.Fatalf("child open changed parent report ownership: %+v", state)
	}
}
