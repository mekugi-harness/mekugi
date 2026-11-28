package launch

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"path/filepath"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

func TestDirectNativeDraftTrimmedOnSubmit(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "trim.sock")
	server := startMockNativeAppServer(t, socket)
	var capturedInput []struct {
		Type         string `json:"type"`
		Text         string `json:"text"`
		TextElements []struct {
			ByteRange struct {
				Start int `json:"start"`
				End   int `json:"end"`
			} `json:"byteRange"`
			Placeholder string `json:"placeholder"`
		} `json:"text_elements"`
	}
	server.mu.Lock()
	server.overrides["turn/start"] = func(params jsontext.Value) any {
		var p struct {
			Input jsontext.Value `json:"input"`
		}
		_ = json.Unmarshal(params, &p)
		_ = json.Unmarshal(p.Input, &capturedInput)
		return map[string]any{"turn": map[string]any{"id": "turn-trim", "status": "inProgress"}}
	}
	server.mu.Unlock()

	conn, err := openDirectNativeConnection(t.Context(), socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Status.State.Text == "Ready" })

	draft := composer.Draft{
		Text:  "  check @main.go  ",
		Files: []composer.Token{{Start: 8, End: 16, Path: "main.go"}},
	}
	if _, err := conn.Send(t.Context(), sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Composer: &sessionapi.NativeComposerInput{
			Intent: sessionpresentation.ComposerIntent{Operation: sessionpresentation.SubmitComposer},
			Draft:  draft,
		},
	}); err != nil {
		t.Fatal(err)
	}
	if len(capturedInput) != 1 || capturedInput[0].Text != "check @main.go" {
		t.Fatalf("expected trimmed input text, got %+v", capturedInput)
	}
	if len(capturedInput[0].TextElements) != 1 ||
		capturedInput[0].TextElements[0].ByteRange.Start != 6 ||
		capturedInput[0].TextElements[0].ByteRange.End != 14 ||
		capturedInput[0].TextElements[0].Placeholder != "@main.go" {
		t.Fatalf("expected shifted file token byte range [6,14], got %+v", capturedInput[0].TextElements)
	}
}

func TestDirectNativeRosterObservedStatusWaitingAndPaused(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "roster-status.sock")
	server := startMockNativeAppServer(t, socket)
	server.mu.Lock()
	server.overrides["thread/subtree/list"] = func(jsontext.Value) any {
		return map[string]any{"data": []any{
			map[string]any{"id": "thr-main", "agentPath": "/root", "status": map[string]any{"type": "active", "activeFlags": []string{"waitingOnApproval"}}},
			map[string]any{"id": "thr-input", "agentPath": "/root/input", "status": map[string]any{"type": "active", "activeFlags": []string{"waitingOnUserInput"}}},
			map[string]any{"id": "thr-paused", "agentPath": "/root/paused", "agentStatus": "interrupted", "status": map[string]any{"type": "idle"}},
			map[string]any{"id": "thr-run", "agentPath": "/root/run", "agentStatus": "running", "status": map[string]any{"type": "active"}},
			map[string]any{"id": "thr-done", "agentPath": "/root/done", "agentStatus": "completed", "status": map[string]any{"type": "idle"}},
			map[string]any{"id": "thr-err", "agentPath": "/root/err", "agentStatus": "errored", "status": map[string]any{"type": "systemError"}},
		}}
	}
	server.mu.Unlock()

	conn, err := openDirectNativeConnection(t.Context(), socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	ev := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.Agents != nil && len(ev.Frame.Agents.Roster) == 6
	})
	got := make(map[string]string, len(ev.Frame.Agents.Roster))
	for _, row := range ev.Frame.Agents.Roster {
		got[row.Agent.Thread] = row.Agent.Status
	}
	want := map[string]string{
		"thr-main":   "waiting",
		"thr-input":  "waiting",
		"thr-paused": "paused",
		"thr-run":    "responding",
		"thr-done":   "answered",
		"thr-err":    "error",
	}
	for id, wantStatus := range want {
		if got[id] != wantStatus {
			t.Errorf("thread %s status = %q, want %q", id, got[id], wantStatus)
		}
	}
}

func TestDirectNativeWillRetryBannerClearsOnResponseProgress(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "retry.sock")
	server := startMockNativeAppServer(t, socket)
	conn, err := openDirectNativeConnection(t.Context(), socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Status.State.Text == "Ready" })

	send := func(method string, params map[string]any) {
		server.send(t, map[string]any{"jsonrpc": "2.0", "method": method, "params": params})
	}
	send("turn/started", map[string]any{"threadId": "thr-main", "turn": map[string]any{"id": "turn-1"}})
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Status.State.Text == "Working" })

	send("error", map[string]any{
		"threadId":  "thr-main",
		"turnId":    "turn-1",
		"willRetry": true,
		"error":     map[string]any{"message": "Reconnecting... 2/5"},
	})
	ev := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Status.State.Text == "Reconnecting... 2/5"
	})
	if ev.Frame.Main != nil && len(ev.Frame.Main.Entries) != 0 {
		t.Fatalf("transient willRetry error should not create transcript error entry: %+v", ev.Frame.Main.Entries)
	}

	// Non-model userShell item must not clear the retry banner.
	send("item/started", map[string]any{
		"threadId": "thr-main",
		"turnId":   "turn-1",
		"item":     map[string]any{"id": "sh-1", "type": "commandExecution", "source": "userShell", "command": "pwd"},
	})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.Main != nil && len(ev.Frame.Main.Entries) == 1
	})
	if ev.Frame.View.Status.State.Text != "Reconnecting... 2/5" {
		t.Fatalf("userShell item cleared retry banner: %q", ev.Frame.View.Status.State.Text)
	}

	// Model response progress clears the retry banner back to Working.
	send("item/agentMessage/delta", map[string]any{
		"threadId": "thr-main",
		"turnId":   "turn-1",
		"itemId":   "msg-1",
		"delta":    "Back online",
	})
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Status.State.Text == "Working"
	})

	// Independent Main + One-off retries do not clobber each other.
	conn.mu.Lock()
	conn.sideThread = "thr-side"
	conn.sideActiveTurn = "side-turn-1"
	conn.mu.Unlock()
	send("error", map[string]any{
		"threadId":  "thr-main",
		"turnId":    "turn-1",
		"willRetry": true,
		"error":     map[string]any{"message": "Main reconnecting... 1/5"},
	})
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Status.State.Text == "Main reconnecting... 1/5"
	})
	send("error", map[string]any{
		"threadId":  "thr-side",
		"turnId":    "side-turn-1",
		"willRetry": true,
		"error":     map[string]any{"message": "Side reconnecting... 1/5"},
	})
	send("item/agentMessage/delta", map[string]any{
		"threadId": "thr-side",
		"turnId":   "side-turn-1",
		"itemId":   "side-msg-1",
		"delta":    "Side recovered",
	})
	// Wait until One-off's delta is actually processed before asserting Main's banner survived.
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		conn.mu.Lock()
		defer conn.mu.Unlock()
		rec := conn.itemsByID[directItemKey{"thr-side", "side-msg-1"}]
		return rec != nil && rec.entry.Text == "Side recovered"
	})
	if ev.Frame.View.Status.State.Text != "Main reconnecting... 1/5" {
		t.Fatalf("side progress cleared main retry banner: %q", ev.Frame.View.Status.State.Text)
	}
}
