package executionobservation

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/moduletest"

	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func TestCompactRecoveryAuthorityAndPrewarm(t *testing.T) {
	p := newManagedMekugiProxy(t)
	attachTestReplayStore(t, p)
	ref, err := moduletest.StoreCompact(t.Context(), p.Retained, "root", "retained handover", `{"sources":["@selected.txt"]}`)
	if err != nil {
		t.Fatal(err)
	}
	recover := func(thread, parent, fork, reference string, warm bool) error {
		req, err := codexresponses.ParseRequest([]byte(fmt.Sprintf(`{"input":[{"type":"compaction","encrypted_content":%q}],"tools":[],"generate":false}`, reference)))
		if err != nil {
			return err
		}
		meta := codexresponses.TurnMetadata{ThreadID: thread, ParentThreadID: parent, ForkedFromThreadID: fork, RequestKind: "turn"}
		if parent != "" {
			meta.SubagentKind = "thread_spawn"
		}
		err = p.prepareCompactInput(t.Context(), &req, "routing", thread, meta, warm)
		if err == nil && (!bytes.Contains(req.Fields["input"], []byte("retained handover")) || bytes.Contains(req.Fields["input"], []byte("selected.txt"))) {
			t.Fatal("summary recovery or metadata separation failed")
		}
		return err
	}
	if err := recover("warm-fork", "", "root", ref, true); err != nil {
		t.Fatal(err)
	}
	if _, found, err := p.Retained.Store.ReadScope("warm-fork"); err != nil || found {
		t.Fatal("prewarm published inheritance", err)
	}
	for _, tc := range []struct{ thread, parent, fork string }{{"root", "", ""}, {"fork", "", "root"}, {"child", "root", ""}, {"fork", "", ""}, {"child", "", ""}} {
		if err := recover(tc.thread, tc.parent, tc.fork, ref, false); err != nil {
			t.Fatal(tc, err)
		}
	}
	if err := recover("foreign", "", "", ref, false); err == nil {
		t.Fatal("foreign reference accepted")
	}
	later, err := moduletest.StoreCompact(t.Context(), p.Retained, "root", "retained handover", "")
	if err != nil {
		t.Fatal(err)
	}
	if err := recover("fork", "", "", later, false); err == nil {
		t.Fatal("frozen fork accepted later allocation")
	}
	if err := recover("child", "", "", later, false); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(p.Retained.Store.Directory(), history.CompactSummaryName(ref))
	if err := os.WriteFile(path, []byte(`{"Version":1,"Reference":"wrong","Summary":"wrong"}`), 0600); err != nil {
		t.Fatal(err)
	}
	if err := recover("root", "", "", ref, false); err == nil {
		t.Fatal("corrupt record accepted")
	}
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if err := recover("root", "", "", ref, false); err == nil {
		t.Fatal("missing record accepted")
	}
}
