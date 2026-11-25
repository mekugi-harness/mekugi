package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"net/http"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
)

type mockNativeAppServer struct {
	mu        sync.Mutex
	conn      *websocket.Conn
	methods   []string
	params    map[string][]jsontext.Value
	responses map[string]jsontext.Value
	ready     chan struct{}
}

func startMockNativeAppServer(t *testing.T, socket string) *mockNativeAppServer {
	t.Helper()
	listener, err := sessionapi.ListenControl(socket)
	if err != nil {
		t.Fatal(err)
	}
	m := &mockNativeAppServer{
		params:    make(map[string][]jsontext.Value),
		responses: make(map[string]jsontext.Value),
		ready:     make(chan struct{}, 4),
	}
	srv := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, nil)
		if err != nil {
			return
		}
		defer conn.CloseNow()
		m.mu.Lock()
		m.conn = conn
		m.mu.Unlock()
		select {
		case m.ready <- struct{}{}:
		default:
		}
		for {
			_, data, err := conn.Read(r.Context())
			if err != nil {
				return
			}
			var msg appserver.Message
			if err := json.Unmarshal(data, &msg); err != nil {
				continue
			}
			if msg.Method == "" && len(msg.ID) > 0 {
				m.mu.Lock()
				m.responses[string(msg.ID)] = slices.Clone(msg.Result)
				m.mu.Unlock()
				continue
			}
			m.mu.Lock()
			m.methods = append(m.methods, msg.Method)
			m.params[msg.Method] = append(m.params[msg.Method], slices.Clone(msg.Params))
			m.mu.Unlock()
			if len(msg.ID) == 0 {
				continue
			}
			result := m.handleCall(msg.Method, msg.Params)
			reply, _ := json.Marshal(map[string]any{
				"jsonrpc": "2.0",
				"id":      msg.ID,
				"result":  result,
			})
			_ = conn.Write(r.Context(), websocket.MessageText, reply)
		}
	})}
	go func() { _ = srv.Serve(listener) }()
	t.Cleanup(func() {
		_ = srv.Close()
		_ = listener.Close()
	})
	return m
}

func (m *mockNativeAppServer) handleCall(method string, params jsontext.Value) any {
	switch method {
	case "initialize":
		return map[string]any{"userAgent": "mekugi-app-server/test"}
	case "thread/start":
		return map[string]any{
			"thread":          map[string]any{"id": "thr-main", "cwd": "/tmp/ws", "name": "Direct Session"},
			"model":           "gpt-5",
			"reasoningEffort": "medium",
			"serviceTier":     "standard",
		}
	case "thread/fork":
		m.mu.Lock()
		count := len(m.params["thread/fork"])
		m.mu.Unlock()
		id := "thr-side"
		if count > 1 {
			id = "thr-btw"
		}
		return map[string]any{
			"thread": map[string]any{"id": id, "cwd": "/tmp/ws"},
		}
	case "thread/subtree/list":
		return map[string]any{
			"data": []map[string]any{
				{"id": "thr-main", "agentPath": "main", "agentRole": "root"},
				{"id": "thr-worker", "agentPath": "main/worker", "agentRole": "explorer"},
			},
		}
	case "thread/changes/read":
		return map[string]any{
			"threadId":       "thr-main",
			"workspace":      "/tmp/ws",
			"totalAdditions": 2,
			"totalDeletions": 1,
			"changes": []map[string]any{
				{"changeId": "amber1", "sequence": 1, "threadId": "thr-main", "sourceKind": "ApplyPatch", "createdAt": 1700000000, "paths": []string{"main.go"}, "incomplete": false},
			},
			"files": []map[string]any{
				{"path": "main.go", "status": "M", "unifiedDiff": "@@ -1 +1,2 @@\n-old\n+new\n+more\n", "additions": 2, "deletions": 1, "isBinary": false, "coverageNotes": []string{}},
			},
		}
	case "thread/queue/list":
		return map[string]any{"items": []any{}}
	case "thread/read":
		return map[string]any{"thread": map[string]any{"id": "thr-worker", "turns": []any{}}}
	case "thread/list":
		return map[string]any{
			"data": []map[string]any{
				{"id": "thr-saved", "name": "Saved session", "cwd": "/tmp/ws", "updatedAt": 1700000001},
			},
		}
	default:
		return map[string]any{}
	}
}

func (m *mockNativeAppServer) send(t *testing.T, payload any) {
	t.Helper()
	m.mu.Lock()
	conn := m.conn
	m.mu.Unlock()
	data, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	if err := conn.Write(t.Context(), websocket.MessageText, data); err != nil {
		t.Fatal(err)
	}
}

func awaitDirectEvent(t *testing.T, conn *directNativeConnection, match func(sessionapi.NativeEvent) bool) sessionapi.NativeEvent {
	t.Helper()
	deadline := time.After(5 * time.Second)
	for {
		ch := make(chan sessionapi.NativeEvent, 1)
		errCh := make(chan error, 1)
		go func() {
			ev, err := conn.Next()
			if err != nil {
				errCh <- err
				return
			}
			ch <- ev
		}()
		select {
		case ev := <-ch:
			if match(ev) {
				return ev
			}
		case err := <-errCh:
			t.Fatalf("unexpected stream error: %v", err)
		case <-deadline:
			t.Fatal("timed out waiting for direct native event")
		}
	}
}

func TestDirectNativeConnectionEndToEnd(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "app-server.sock")
	mock := startMockNativeAppServer(t, socket)

	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, []string{"--yolo"}, "", "")
	if err != nil {
		t.Fatalf("openDirectNativeConnection: %v", err)
	}
	defer conn.Close()

	// 1. Initial frame carries native thread, roster/family, and saved changes.
	initial := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Main.ID == "thr-main" &&
			ev.Frame.Family != nil && len(ev.Frame.Family.Threads) == 2 &&
			ev.Frame.Review != nil && len(ev.Frame.Review.Files) == 1
	})
	if initial.Frame.Review.Files[0].Path != "main.go" {
		t.Fatalf("unexpected saved change file: %+v", initial.Frame.Review.Files)
	}

	// 2. Live transcript items, command output hydration, and server-owned inline card (MEKUGI-788 style).
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"method":  "turn/started",
		"params":  map[string]any{"threadId": "thr-main", "turn": map[string]any{"id": "turn-1"}},
	})
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"method":  "item/started",
		"params": map[string]any{
			"threadId": "thr-main", "turnId": "turn-1",
			"item": map[string]any{"id": "cmd-1", "type": "commandExecution", "command": "echo hi", "status": "inProgress"},
		},
	})
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"method":  "item/commandExecution/outputDelta",
		"params":  map[string]any{"threadId": "thr-main", "turnId": "turn-1", "itemId": "cmd-1", "delta": "hi\n"},
	})
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"method":  "item/completed",
		"params": map[string]any{
			"threadId": "thr-main", "turnId": "turn-1",
			"item": map[string]any{
				"id":     "card-788",
				"type":   "serverInlineCard",
				"title":  "Server Card",
				"body":   "Rendered directly from native app-server",
				"status": "completed",
			},
		},
	})
	ev := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.Main != nil && len(ev.Frame.Main.Entries) >= 2
	})
	var cmdSeq uint64
	var foundCard bool
	for _, e := range ev.Frame.Main.Entries {
		if len(e.Values) > 0 && e.Values[0].Verb == "Run" {
			cmdSeq = e.Seq
		}
		if len(e.Values) > 0 && e.Values[0].Verb == "Server Card" && e.Text == "Rendered directly from native app-server" {
			foundCard = true
		}
	}
	if cmdSeq == 0 || !foundCard {
		t.Fatalf("expected command and server-owned inline card, got entries=%+v", ev.Frame.Main.Entries)
	}

	// Hydrate command output directly from native output buffer.
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Output: &sessionapi.NativeOutputDemand{Demands: []activity.OutputDemand{{Source: cmdSeq, Output: 1}}},
	}); err != nil {
		t.Fatal(err)
	}
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return len(ev.Frame.MainOutput) == 1 && len(ev.Frame.MainOutput[0].View.Lines) > 0 && ev.Frame.MainOutput[0].View.Lines[0] == "hi"
	})

	// 3. Steering & queueing while turn is active.
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Composer: &sessionapi.NativeComposerInput{
			Intent: sessionpresentation.ComposerIntent{Operation: sessionpresentation.SubmitComposer},
			Draft:  composer.Draft{Text: "steer this"},
		},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Composer: &sessionapi.NativeComposerInput{
			Intent: sessionpresentation.ComposerIntent{Operation: sessionpresentation.QueueComposer},
			Draft:  composer.Draft{Text: "queue this"},
		},
	}); err != nil {
		t.Fatal(err)
	}

	// 4. Approval and question/secret server requests.
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"id":      "req-approve-1",
		"method":  "item/commandExecution/requestApproval",
		"params":  map[string]any{"threadId": "thr-main", "turnId": "turn-1", "itemId": "cmd-2", "command": "rm -rf /tmp/x"},
	})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return len(ev.Frame.Approvals) == 1 && ev.Frame.Approvals[0].ID == `"req-approve-1"`
	})
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Approval: &sessionpresentation.ApprovalIntent{Request: `"req-approve-1"`, Choice: 0},
	}); err != nil {
		t.Fatal(err)
	}

	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"id":      "req-secret-1",
		"method":  "shellGuard/requestSecret",
		"params":  map[string]any{"threadId": "thr-main", "prompt": "sudo password"},
	})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return len(ev.Frame.Questions) == 1 && ev.Frame.Questions[0].ID == `"req-secret-1"`
	})
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Answers: &sessionapi.NativeAnswers{
			Call:    `"req-secret-1"`,
			Answers: []sessionapi.NativeQuestionAnswer{{AnswerInput: prompt.AnswerInput{Question: "secret"}, Secret: "topsecret"}},
		},
	}); err != nil {
		t.Fatal(err)
	}

	// 5. Settings, rewind, Side, One-off (/btw), and Display selection (Herdr panel).
	for _, cmd := range []string{"/model gpt-5.1", "/effort high", "/rewind", "/side check this", "/btw quick question"} {
		if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{Command: new(cmd)}); err != nil {
			t.Fatalf("command %q: %v", cmd, err)
		}
	}
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Display: &sessionpresentation.DisplayIntent{Thread: sessionpresentation.Thread{ID: "thr-worker", Caller: "main/worker"}},
	}); err != nil {
		t.Fatal(err)
	}
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Displayed.ID == "thr-worker" && ev.Frame.View.Displayed.Caller == "main/worker"
	})

	// Verify all native RPC methods were invoked directly on mekugi-app-server.
	mock.mu.Lock()
	methods := slices.Clone(mock.methods)
	approveResp := string(mock.responses[`"req-approve-1"`])
	secretResp := string(mock.responses[`"req-secret-1"`])
	mock.mu.Unlock()
	for _, want := range []string{"initialize", "thread/start", "thread/subtree/list", "thread/changes/read", "turn/steer", "thread/queue/add", "thread/settings/update", "thread/revert", "thread/fork", "turn/start"} {
		if !slices.Contains(methods, want) {
			t.Fatalf("missing direct native RPC %q; observed=%v", want, methods)
		}
	}
	if !strings.Contains(approveResp, "accept") {
		t.Fatalf("unexpected approval response: %s", approveResp)
	}
	if !strings.Contains(secretResp, "topsecret") {
		t.Fatalf("unexpected secret response: %s", secretResp)
	}
}
