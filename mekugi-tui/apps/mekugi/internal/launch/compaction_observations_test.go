package launch

import (
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func TestCompactionMeasurementsRetainRestoreReplay(t *testing.T) {
	p := newManagedMekugiProxy(t)
	attachTestReplayStore(t, p)
	u := newAppServerSessionTestUI(t, "/workspace/replay")
	u.retainedSession = history.Session{Store: p.retained.Store, Thread: "main", Context: t.Context()}
	at := time.UnixMilli(replayTestEpoch)
	u.clock = func() time.Time { return at }
	appServerTestNotify(t, u, "turn/started", map[string]any{"threadId": "main", "turn": map[string]any{"id": "turn"}})
	agent := u.attached.session.Agent("/root")
	agent.ContextKnown, agent.ContextTokens, agent.ContextWindow = true, 120000, 200000
	appServerTestNotify(t, u, "item/started", map[string]any{"threadId": "main", "turnId": "turn", "item": binding.ThreadItem{ID: "compact", Type: "contextCompaction", Status: "inProgress"}})
	at = at.Add(12 * time.Second)
	appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "turn", "item": binding.ThreadItem{ID: "compact", Type: "contextCompaction", Status: "completed"}})
	retained, _, err := p.retained.Store.ReadCompaction("main", "compact")
	if err != nil || retained == nil || retained.Context == nil || retained.Context.Tokens != 120000 || retained.ElapsedNS == nil || time.Duration(*retained.ElapsedNS) != 12*time.Second {
		t.Fatalf("missing completed boundary measurements: %+v, %v", retained, err)
	}
	if foreign, _, err := p.retained.Store.ReadCompaction("other", "compact"); err != nil || foreign != nil {
		t.Fatalf("foreign measurement: %+v, %v", foreign, err)
	}
	want := u.attached.source.Entries[0].Values[0].Label
	if retained.Method != observation.ContextReset || !strings.HasPrefix(want, "Context reset") || u.attached.source.Entries[0].Values[0].Disclosure() != nil {
		t.Fatal("live reset lost method or exposed a summary")
	}
	restored := newAppServerSessionTestUI(t, "/workspace/replay")
	restored.retainedSession = u.retainedSession
	restored.restoreHistory([]binding.HistoryTurn{{ID: "turn", Status: "completed", Items: []binding.ThreadItem{{ID: "compact", Type: "contextCompaction", Status: "completed"}}}})
	applyCompactionObservations(&restored.attached, &appServerActivityRestore{}, restored.retainedSession.Store.ReadCompaction)
	if got := restored.attached.source.Entries[0].Values[0].Label; got != want {
		t.Fatalf("restore label %q, live %q", got, want)
	}
	if restored.attached.source.Entries[0].Values[0].Disclosure() != nil {
		t.Fatal("restored reset exposed a summary")
	}
}
