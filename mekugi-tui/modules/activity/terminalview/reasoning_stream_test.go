package terminalview

import (
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
)

func TestReasoningStreamReservesAndDrainsCompletion(t *testing.T) {
	at := time.Unix(100, 0)
	body := strings.Repeat("A public summary line.\n", 20)
	entry := observation.TextUpdate{Ref: observation.ItemRef{ThreadID: "main", TurnID: "turn", ItemID: "summary"}, Kind: "reasoning", Phase: "summary", Text: body, Observed: at, Live: true, Sequence: 10}
	var stream ReasoningStream
	reserved := stream.Queue([]observation.TextUpdate{entry})
	if len(reserved) != 1 || reserved[0].Text != "" || reserved[0].Phase != "queued-summary" {
		t.Fatalf("reservation: %+v", reserved)
	}
	first := stream.Advance(at)
	if len(first) != 1 || first[0].Text == body || first[0].Text == "" {
		t.Fatalf("unpaced first frame: %+v", first)
	}
	entry.Phase, entry.Thought, entry.Sequence = "item/completed", 2*time.Second, 11
	if shown := stream.Queue([]observation.TextUpdate{entry}); len(shown) != 0 {
		t.Fatalf("completion bypassed backlog: %+v", shown)
	}
	var done observation.TextUpdate
	for i := range 100 {
		for _, update := range stream.Advance(at.Add(time.Duration(i) * time.Second)) {
			if update.Phase == "item/completed" {
				done = update
			}
		}
		if done.Phase != "" {
			break
		}
	}
	if done.Text != body || done.Thought != 2*time.Second || done.Completed.IsZero() {
		t.Fatalf("completion lost source or painted time: %+v", done)
	}
	if updates := stream.Advance(at.Add(time.Hour)); len(updates) != 0 {
		t.Fatalf("completed stream reopened: %+v", updates)
	}
}

