package launch

import (
	json "encoding/json/v2"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const replayTestEpoch int64 = 1700000000000

func replayTestRecord(kind string, at int64, payload map[string]any) map[string]any {
	return map[string]any{"type": kind, "timestamp": time.UnixMilli(at), "payload": payload}
}

func replayTestMeta(id string) map[string]any {
	return replayTestRecord("session_meta", replayTestEpoch, map[string]any{"id": id, "cwd": "/workspace/replay"})
}

func replayTestItem(thread, turn string, start, end int64, item map[string]any) map[string]any {
	return replayTestRecord("event_msg", end, map[string]any{
		"type": "item_completed", "thread_id": thread, "turn_id": turn,
		"started_at_ms": start, "completed_at_ms": end, "item": item,
	})
}

func replayTestWrite(t *testing.T, dir, name string, records ...map[string]any) string {
	t.Helper()
	var data strings.Builder
	for _, record := range records {
		b, err := json.Marshal(&record)
		if err != nil {
			t.Fatal(err)
		}
		data.Write(b)
		data.WriteByte('\n')
	}
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, []byte(data.String()), 0600); err != nil {
		t.Fatal(err)
	}
	return path
}
