package capture

import (
	json "encoding/json/v2"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func timingStore(t *testing.T) (*history.Service, *history.Store) {
	t.Helper()
	owner, err := history.NewService([]recordkind.Spec{TimingRecord})
	if err != nil {
		t.Fatal(err)
	}
	store, err := owner.Open(t.Context(), t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return owner, store
}

func TestInspectRetainedRequestTiming(t *testing.T) {
	owner, store := timingStore(t)
	missing := owner.Attach(filepath.Join(t.TempDir(), "absent"))
	data, err := new(Service).InspectTiming(t.Context(), missing, "historical")
	if err != nil || !strings.Contains(string(data), `"state":"unavailable"`) {
		t.Fatalf("historical evidence=%s error=%v", data, err)
	}
	if _, err := os.Stat(missing.Directory()); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("inspection created storage")
	}
	at := time.Now()
	window := timingWindow{Version: 1, Thread: "thread", Requests: []RequestTiming{{
		CaptureID: "retained", Sequence: 1, Started: at, Outcome: "completed", Completed: &TimingPhase{At: at},
	}}}
	data, err = json.Marshal(window)
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(store.Directory(), timingName("thread"))
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
	report, err := new(Service).InspectTiming(t.Context(), store, "thread")
	if err != nil || !strings.Contains(string(report), `"state":"observed_window"`) {
		t.Fatalf("retained timing: %s, %v", report, err)
	}
	for _, mutation := range []string{"thread", "phase", "version", "outcome", "duplicate"} {
		t.Run(mutation, func(t *testing.T) {
			copy := window
			copy.Requests = append([]RequestTiming(nil), window.Requests...)
			switch mutation {
			case "thread":
				copy.Thread = "another-thread"
			case "version":
				copy.Version++
			case "phase":
				copy.Requests[0].Completed = &TimingPhase{At: time.Now(), OffsetMillis: -1}
			case "outcome":
				copy.Requests[0].Outcome = "private provider error"
			case "duplicate":
				copy.Requests = append(copy.Requests, copy.Requests[0])
			}
			data, err := json.Marshal(&copy)
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(store.Directory(), timingName("thread")), data, 0600); err != nil {
				t.Fatal(err)
			}
			if _, _, err := readTimingWindow(store, "thread"); err == nil {
				t.Fatal("invalid evidence accepted by record decoder")
			}
			data, err = new(Service).InspectTiming(t.Context(), store, "thread")
			if err != nil || !strings.Contains(string(data), `"state":"error"`) || strings.Contains(string(data), `"requests"`) {
				t.Fatalf("invalid evidence was not isolated: %s %v", data, err)
			}
		})
	}
}
