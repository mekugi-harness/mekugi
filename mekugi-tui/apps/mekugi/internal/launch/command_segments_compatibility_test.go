package launch

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	sessionhistory "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

// Keep this wire fixture independent of the current encoder: these are the
// version-1 replay fields used before command segments had their own store.
// The numeric array is SHA-256("one\n"), not encoded per-segment output.
func commandSegmentsCompatEnvelope(workspace, id string) string {
	return fmt.Sprintf(`{
		"Version":1,"Workspace":%q,"CallID":%q,"Commentary":false,
		"History":{"CommandSegments":{
			"Command":"bash -lc 'echo one; false'","Exit":1,
			"Output":[44,139,8,218,92,230,3,152,225,241,154,240,229,220,204,116,77,242,116,184,38,171,229,133,234,186,104,197,37,67,72,6],
			"Parts":[
				{"Timing":{"started":"2025-01-02T03:04:05Z","ended":"2025-01-02T03:04:06Z","elapsed_ns":1000000000},"Source":"echo one","Skipped":false,"Exit":0,"Output":"one"},
				{"Source":"false","Skipped":false,"Exit":1,"Output":""}
			]
		}}
	}`, workspace, id)
}

func writeCommandSegmentsCompatEnvelope(t *testing.T, store *mekugiReplayStore, workspace, id, wire string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(store.directory, sessionhistory.Name(workspace, id, false)), []byte(wire), 0600); err != nil {
		t.Fatal(err)
	}
}
