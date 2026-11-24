package launch

import (
	"fmt"
	"strings"
	"testing"

	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"

	segmenttransport "github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/transport"
)

func TestAppServerTrackedEditCommandOutcomeBeforeHostExit(t *testing.T) {
	t.Parallel()
	u, hub := newTrackedAppServerUI(t)
	script := sedInPlace() + " 's/absent/after/' source.go; go test ./..."
	item := map[string]any{"id": "edit", "type": "commandExecution", "command": "/bin/bash -lc " + quoteShellWord(script), "status": "inProgress"}
	appServerTestNotify(t, u, "item/started", map[string]any{"threadId": "main", "turnId": "turn", "item": item})
	report := dialExecTrackReport(t, hub, script)
	report.send(segmenttransport.Message{Type: segmenttransport.Begin, Index: 0})
	awaitMain(t, u, "via sed · requested")
	report.send(segmenttransport.Message{Type: segmenttransport.End, Index: 0, Code: new(0)}, segmenttransport.Message{Type: segmenttransport.Begin, Index: 1})
	awaitMain(t, u, "via sed · ran")
	shown := awaitMain(t, u, "Running go test")
	if strings.Contains(shown, "requested") || !strings.Contains(shown, "via sed · ran") || u.attached.session.Commands()[[3]string{"main", "turn", "edit"}] == nil {
		t.Fatalf("segment completion changed host lifecycle:\n%s", shown)
	}
	// A zero exit also describes a no-op. Without file evidence, show the
	// command outcome but do not invent a confirmed edit or applied counts.
	for _, block := range u.testActivity(u.shell.Transcript).blocks(0) {
		if block.Verb == "Edit" {
			if _, _, _, _, counts := activityui.EditStat(block.Label); counts || block.GroupHeader == "Edited" || block.EditOutcome != "ran" {
				t.Fatalf("command completion confirmed a file change: %+v", block)
			}
		}
	}
}

func TestAppServerTrackedEditFailureAndSkip(t *testing.T) {
	t.Parallel()
	u, hub := newTrackedAppServerUI(t)
	script := sedInPlace() + " 's/a/b/' missing && " + sedInPlace() + " 's/b/c/' skipped"
	item := map[string]any{"id": "edit", "type": "commandExecution", "command": "/bin/bash -lc " + quoteShellWord(script), "status": "inProgress"}
	appServerTestNotify(t, u, "item/started", map[string]any{"threadId": "main", "turnId": "turn", "item": item})
	report := dialExecTrackReport(t, hub, script)
	report.send(segmenttransport.Message{Type: segmenttransport.Begin, Index: 0}, segmenttransport.Message{Type: segmenttransport.End, Index: 0, Code: new(2)}, segmenttransport.Message{Type: segmenttransport.Done, Code: new(2)}, segmenttransport.Message{Type: segmenttransport.Closed})
	report.conn.Close()
	item["status"], item["exitCode"] = "failed", 2
	appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "turn", "item": item})
	shown := awaitMain(t, u, "via sed · skipped")
	if strings.Contains(shown, "requested") || strings.Contains(shown, "completed") || !strings.Contains(shown, "failed") || !strings.Contains(shown, "exit 2") {
		t.Fatalf("failed/skipped edits mislabeled:\n%s", shown)
	}
}

func TestAppServerTrackedSkippedEditBeforeHostExit(t *testing.T) {
	t.Parallel()
	u, hub := newTrackedAppServerUI(t)
	script := "false && " + sedInPlace() + " 's/a/b/' skipped; go test ./..."
	item := map[string]any{"id": "edit", "type": "commandExecution", "command": "/bin/bash -lc " + quoteShellWord(script), "status": "inProgress"}
	appServerTestNotify(t, u, "item/started", map[string]any{"threadId": "main", "turnId": "turn", "item": item})
	report := dialExecTrackReport(t, hub, script)
	report.send(segmenttransport.Message{Type: segmenttransport.Begin, Index: 0}, segmenttransport.Message{Type: segmenttransport.End, Index: 0, Code: new(1)}, segmenttransport.Message{Type: segmenttransport.Begin, Index: 2})
	awaitMain(t, u, "via sed · skipped")
	shown := awaitMain(t, u, "Running")
	if !strings.Contains(shown, "Running go test") || strings.Contains(shown, "requested") || strings.Contains(shown, "skipped · skipped") {
		t.Fatalf("skipped edit waited for host exit:\n%s", shown)
	}
}

func TestExecTrackRapidSegmentsKeepOwnOutput(t *testing.T) {
	t.Parallel()
	shell := newExecTrackShell(t)
	var script strings.Builder
	for i := range 128 {
		fmt.Fprintf(&script, "printf '%d\\n';\n", i)
	}
	key := [3]string{"thread", "turn", "rapid"}
	shell.hub.start(key, "/bin/bash -lc "+quoteShellWord(script.String()))
	result := runExecTrackShell(t, shell.env, script.String())
	view := shell.awaitView(t, key)
	if result.code != 0 || !view.complete || len(view.segments) != 128 {
		t.Fatalf("rapid list incomplete: code=%d complete=%v segments=%d", result.code, view.complete, len(view.segments))
	}
	for i, segment := range view.segments {
		if len(segment.Tail) != 1 || segment.Tail[0] != fmt.Sprint(i) {
			t.Fatalf("segment %d borrowed or lost output: %q", i, segment.Tail)
		}
	}
}
