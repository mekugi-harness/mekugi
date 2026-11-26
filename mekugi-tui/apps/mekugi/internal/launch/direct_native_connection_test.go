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
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
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
	listener, err := listenPrivateUnix(socket)
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
		return map[string]any{
			"thread": map[string]any{
				"id": "thr-main",
				"turns": []map[string]any{
					{
						"id": "turn-1",
						"items": []map[string]any{
							{"id": "cmd-1", "type": "commandExecution", "command": "echo hi", "status": "completed", "aggregatedOutput": "hi\n"},
							{"id": "card-788", "type": "serverInlineCard", "title": "Server Card", "body": "Rendered directly from native app-server", "status": "completed"},
						},
					},
				},
			},
		}
	case "thread/list":
		return map[string]any{
			"data": []map[string]any{
				{"id": "thr-saved", "name": "Saved session", "cwd": "/tmp/ws", "updatedAt": 1700000001},
			},
		}
	case "skills/list":
		return map[string]any{
			"data": []map[string]any{
				{
					"cwd": "/tmp/ws",
					"skills": []map[string]any{
						{"name": "review-pr", "path": "/tmp/ws/.agents/skills/review-pr/SKILL.md", "shortDescription": "Review a pull request", "enabled": true},
						{"name": "disabled-skill", "path": "/tmp/ws/.agents/skills/disabled/SKILL.md", "description": "Disabled", "enabled": false},
					},
				},
			},
		}
	case "fuzzyFileSearch":
		return map[string]any{
			"files": []map[string]any{
				{"root": "/tmp/ws", "path": "cmd/main.go", "match_type": "file", "file_name": "main.go", "score": 100},
			},
		}
	case "model/list":
		return map[string]any{
			"data": []map[string]any{
				{
					"id":          "gpt-6.2-sol",
					"model":       "gpt-6.2-sol",
					"displayName": "GPT-6.2 Sol",
					"description": "Fast coding model",
					"supportedReasoningEfforts": []map[string]any{
						{"reasoningEffort": "low", "description": "Quick"},
						{"reasoningEffort": "high", "description": "Deep"},
					},
					"serviceTiers": []map[string]any{
						{"id": "priority", "displayName": "Fast", "description": "Priority tier"},
					},
				},
				{
					"id":          "hidden-model",
					"model":       "hidden-model",
					"displayName": "Hidden",
					"hidden":      true,
				},
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
	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, []string{"--yolo", "-p", "custom-prof"}, "", "custom-prof")
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
	if initial.Frame.Main != initial.Frame.Agents {
		t.Fatalf("expected Main and Agents to share publication when Displayed == Main")
	}
	mock.mu.Lock()
	startParams := mock.params["thread/start"]
	mock.mu.Unlock()
	if len(startParams) != 1 {
		t.Fatalf("expected 1 thread/start call, got %d", len(startParams))
	}
	var startBody struct {
		Family map[string]jsontext.Value `json:"family"`
	}
	if err := json.Unmarshal(startParams[0], &startBody); err != nil {
		t.Fatalf("unmarshal thread/start: %v", err)
	}
	if _, hasHome := startBody.Family["home"]; hasHome {
		t.Fatalf("thread/start family must not send home: %s", startParams[0])
	}
	if len(startBody.Family["environment"]) == 0 || decodeJSONString(startBody.Family["profile"]) != "custom-prof" {
		t.Fatalf("thread/start family missing environment or profile: %s", startParams[0])
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

	// 3. Steering & queueing while turn is active (preserving image & selection attachments).
	steerDraft := composer.Draft{
		Text:         "steer [img] this",
		Images:       []composer.Token{{Start: 6, End: 11, Path: "/tmp/shot.png"}},
		AnswerImages: []string{"/tmp/ans.png"},
	}
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Composer: &sessionapi.NativeComposerInput{
			Intent: sessionpresentation.ComposerIntent{Operation: sessionpresentation.SubmitComposer},
			Draft:  steerDraft,
		},
	}); err != nil {
		t.Fatal(err)
	}
	queueDraft := composer.Draft{
		Text:             "queue this",
		AnswerSelections: "selected snippet",
	}
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Composer: &sessionapi.NativeComposerInput{
			Intent: sessionpresentation.ComposerIntent{Operation: sessionpresentation.QueueComposer},
			Draft:  queueDraft,
		},
	}); err != nil {
		t.Fatal(err)
	}

	// 4. Approvals (shellGuard with executable/argv, permissions) and secret prompts.
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"id":      "req-approve-1",
		"method":  "shellGuard/requestApproval",
		"params": map[string]any{
			"threadId": "thr-main", "turnId": "turn-1", "itemId": "cmd-2",
			"kind": "remoteWrite", "cwd": "/tmp/ws", "executable": "git", "argv": []string{"push", "origin", "main"},
		},
	})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return len(ev.Frame.Approvals) == 1 && ev.Frame.Approvals[0].ID == `"req-approve-1"`
	})
	if ev.Frame.Approvals[0].Subject != "git push origin main" {
		t.Fatalf("expected shellGuard approval subject to include executable+argv, got %q", ev.Frame.Approvals[0].Subject)
	}
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Approval: &sessionpresentation.ApprovalIntent{Request: `"req-approve-1"`, Choice: 0},
	}); err != nil {
		t.Fatal(err)
	}

	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"id":      "req-perm-1",
		"method":  "item/permissions/requestApproval",
		"params": map[string]any{
			"threadId": "thr-main", "turnId": "turn-1", "itemId": "perm-1",
			"permissions": map[string]any{"network": map[string]any{"enabled": true}},
		},
	})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return len(ev.Frame.Approvals) == 1 && ev.Frame.Approvals[0].ID == `"req-perm-1"`
	})
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Approval: &sessionpresentation.ApprovalIntent{Request: `"req-perm-1"`, Choice: 1},
	}); err != nil {
		t.Fatal(err)
	}

	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"id":      "req-secret-1",
		"method":  "shellGuard/requestSecret",
		"params":  map[string]any{"threadId": "thr-main", "kind": "sudoPassword", "executable": "sudo", "argv": []string{"apt", "update"}},
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
	mock.send(t, map[string]any{
		"jsonrpc": "2.0",
		"id":      "req-secret-2",
		"method":  "shellGuard/requestSecret",
		"params":  map[string]any{"threadId": "thr-main", "kind": "gpgPassphrase", "executable": "gpg", "argv": []string{"--sign"}},
	})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return len(ev.Frame.Questions) == 1 && ev.Frame.Questions[0].ID == `"req-secret-2"`
	})
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
		Answers: &sessionapi.NativeAnswers{
			Call:       `"req-secret-2"`,
			Incomplete: true,
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

	// Verify history reload preserves visible transcript items and surviving Seq identities.
	conn.reloadThreadTranscript(ctx, "thr-main")
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.Main != nil && len(ev.Frame.Main.Entries) == 2
	})
	if ev.Frame.Main.Entries[0].Seq != cmdSeq {
		t.Fatalf("expected reload to preserve item Seq %d, got %d", cmdSeq, ev.Frame.Main.Entries[0].Seq)
	}

	// Verify resume picker scopes thread/list to current cwd.
	if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{Command: new("/resume")}); err != nil {
		t.Fatal(err)
	}

	// Verify all native RPC methods were invoked directly on mekugi-app-server.
	mock.mu.Lock()
	methods := slices.Clone(mock.methods)
	approveResp := string(mock.responses[`"req-approve-1"`])
	permResp := string(mock.responses[`"req-perm-1"`])
	secretResp := string(mock.responses[`"req-secret-1"`])
	secretDeclineResp := string(mock.responses[`"req-secret-2"`])
	steerParams := slices.Clone(mock.params["turn/steer"])
	queueParams := slices.Clone(mock.params["thread/queue/add"])
	revertParams := slices.Clone(mock.params["thread/revert"])
	forkParams := slices.Clone(mock.params["thread/fork"])
	turnStartParams := slices.Clone(mock.params["turn/start"])
	listParams := slices.Clone(mock.params["thread/list"])
	turnsListParams := slices.Clone(mock.params["thread/turns/list"])
	mock.mu.Unlock()
	for _, want := range []string{"initialize", "thread/start", "thread/subtree/list", "thread/changes/read", "turn/steer", "thread/queue/add", "thread/settings/update", "thread/revert", "thread/fork", "turn/start", "thread/list"} {
		if !slices.Contains(methods, want) {
			t.Fatalf("missing direct native RPC %q; observed=%v", want, methods)
		}
	}
	if !strings.Contains(approveResp, `"decision":"accept"`) || strings.Contains(approveResp, `"allow"`) {
		t.Fatalf("unexpected shellGuard approval response: %s", approveResp)
	}
	if !strings.Contains(permResp, `"scope":"session"`) || !strings.Contains(permResp, `"network"`) {
		t.Fatalf("unexpected permissions approval response: %s", permResp)
	}
	if !strings.Contains(secretResp, `"decision":"provide"`) || !strings.Contains(secretResp, `"secret":"topsecret"`) {
		t.Fatalf("unexpected secret response: %s", secretResp)
	}
	if !strings.Contains(secretDeclineResp, `"decision":"decline"`) {
		t.Fatalf("unexpected declined secret response: %s", secretDeclineResp)
	}
	if len(steerParams) == 0 || !strings.Contains(string(steerParams[0]), "/tmp/shot.png") || !strings.Contains(string(steerParams[0]), "/tmp/ans.png") {
		t.Fatalf("turn/steer dropped image attachments: %s", steerParams)
	}
	if len(queueParams) == 0 || !strings.Contains(string(queueParams[0]), `"clientUserMessageId"`) || !strings.Contains(string(queueParams[0]), "selected snippet") {
		t.Fatalf("thread/queue/add missing clientUserMessageId or selection: %s", queueParams)
	}
	if len(revertParams) == 0 || !strings.Contains(string(revertParams[0]), `"beforeTurnId":"turn-1"`) {
		t.Fatalf("thread/revert missing beforeTurnId: %s", revertParams)
	}
	if len(forkParams) < 2 || !strings.Contains(string(forkParams[1]), `"sandbox":"read-only"`) || !strings.Contains(string(forkParams[1]), `"approvalPolicy":"never"`) {
		t.Fatalf("/btw fork missing read-only sandbox or approvalPolicy: %s", forkParams)
	}
	if len(turnStartParams) < 2 || !strings.Contains(string(turnStartParams[1]), `"environments":[]`) {
		t.Fatalf("/btw turn/start missing empty environments: %s", turnStartParams)
	}
	if len(listParams) == 0 || !strings.Contains(string(listParams[0]), `"cwd":"/tmp/ws"`) {
		t.Fatalf("thread/list missing cwd workspace filter: %s", listParams)
	}
	if len(turnsListParams) == 0 || !strings.Contains(string(turnsListParams[0]), `"sortDirection":"desc"`) {
		t.Fatalf("thread/turns/list missing sortDirection=desc: %s", turnsListParams)
	}
}

func TestDirectNativeNewClearsActiveTurnBeforeRewind(t *testing.T) {
	conn := &directNativeConnection{
		mainThread:    "thr-a",
		activeTurn:    "turn-a-1",
		threadCallers: map[string]string{"thr-a": "main"},
	}
	conn.applyThreadBootstrapLocked("thr-b", "/tmp/ws", "Fresh Session", "gpt-5", "medium", "standard")
	if conn.mainThread != "thr-b" {
		t.Fatalf("expected mainThread=thr-b, got %q", conn.mainThread)
	}
	if conn.activeTurn != "" {
		t.Fatalf("expected activeTurn cleared on main thread switch, got %q", conn.activeTurn)
	}
}

func TestDirectNativeProjectionPopulatesEntryValues(t *testing.T) {
	conn := &directNativeConnection{
		wire:          &nativeAppServerWire{userAgent: "mekugi/0.172.0"},
		mainThread:    "thr-main",
		items:         make(map[string][]*directItemRecord),
		itemsByID:     make(map[string]*directItemRecord),
		threadCallers: map[string]string{"thr-main": "main"},
		events:        make(chan sessionapi.NativeEvent, 16),
	}

	conn.handleNotification(appserver.Message{
		Method: "item/agentMessage/delta",
		Params: jsontext.Value(`{"threadId":"thr-main","turnId":"turn-1","itemId":"msg-1","delta":"Hello "}`),
	})
	rec := conn.itemsByID["msg-1"]
	if rec == nil || len(rec.entry.Values) != 1 || rec.entry.Values[0].Kind != "final" || rec.entry.Values[0].Body != "Hello " {
		t.Fatalf("streaming agentMessage did not project Values: %+v", rec)
	}

	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{"threadId":"thr-main","turnId":"turn-1","item":{"type":"agentMessage","id":"msg-1","text":"Hello world"}}`),
	})
	if len(rec.entry.Values) != 1 || rec.entry.Values[0].Kind != "final" || rec.entry.Values[0].Body != "Hello world" || rec.entry.Values[0].Owner != "main" {
		t.Fatalf("completed agentMessage did not project Values: %+v", rec.entry.Values)
	}

	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{"threadId":"thr-main","turnId":"turn-1","item":{"type":"reasoning","id":"rsn-1","summary":["Thinking step"]}}`),
	})
	rsn := conn.itemsByID["rsn-1"]
	if rsn == nil || len(rsn.entry.Values) == 0 {
		t.Fatalf("reasoning item did not project Values: %+v", rsn)
	}

	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{"threadId":"thr-main","turnId":"turn-1","item":{"type":"plan","id":"pln-1","text":"1. Fix bug"}}`),
	})
	pln := conn.itemsByID["pln-1"]
	if pln == nil || len(pln.entry.Values) != 1 || pln.entry.Values[0].Body != "1. Fix bug" {
		t.Fatalf("plan item did not project Values: %+v", pln)
	}
}

func TestDirectNativeSlashCompletionAndMenuAccept(t *testing.T) {
	conn := &directNativeConnection{
		wire:          &nativeAppServerWire{userAgent: "mekugi/0.172.0"},
		mainThread:    "thr-main",
		items:         make(map[string][]*directItemRecord),
		itemsByID:     make(map[string]*directItemRecord),
		threadCallers: map[string]string{"thr-main": "main"},
		events:        make(chan sessionapi.NativeEvent, 16),
	}

	// Typing "/" opens slash command menu.
	_, err := conn.Send(context.Background(), sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Completion: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 1},
			Draft:  composer.Draft{Text: "/"},
		},
	})
	if err != nil {
		t.Fatalf("completion failed: %v", err)
	}
	ev := <-conn.events
	if ev.Frame.View.Menu == nil || ev.Frame.View.Menu.Kind != sessionpresentation.MenuCompletion || len(ev.Frame.View.Menu.Choices) == 0 {
		t.Fatalf("expected slash completion menu, got %+v", ev.Frame.View.Menu)
	}
	if ev.Frame.CompletionTarget != (conversation.EditorTarget{Revision: 1}) {
		t.Fatalf("expected CompletionTarget revision 1 so renderer keeps menu visible, got %+v", ev.Frame.CompletionTarget)
	}
	menuID := ev.Frame.View.Menu.ID

	// Filtering "/res" narrows to /resume.
	_, _ = conn.Send(context.Background(), sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Completion: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 2},
			Draft:  composer.Draft{Text: "/res"},
		},
	})
	ev = <-conn.events
	if ev.Frame.View.Menu == nil || len(ev.Frame.View.Menu.Choices) != 1 || ev.Frame.View.Menu.Choices[0].Name != "/resume" {
		t.Fatalf("expected /resume match, got %+v", ev.Frame.View.Menu)
	}
	if ev.Frame.CompletionTarget != (conversation.EditorTarget{Revision: 2}) {
		t.Fatalf("expected CompletionTarget revision 2, got %+v", ev.Frame.CompletionTarget)
	}

	// Accepting emits Delete + Insert effects targeting the editor revision sequence.
	_, _ = conn.Send(context.Background(), sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Menu: &sessionpresentation.MenuAction{
			Thread: "thr-main",
			Menu:   menuID,
			Kind:   sessionpresentation.MenuAccept,
			Choice: completion.ChoiceID{Name: "/resume"},
		},
		Editor: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 2},
			Draft:  composer.Draft{Text: "/res"},
		},
	})
	ev = <-conn.events
	if ev.Frame.View.Menu != nil {
		t.Fatalf("expected menu closed after accept, got %+v", ev.Frame.View.Menu)
	}
	if len(ev.Frame.Effects) != 2 {
		t.Fatalf("expected 2 editor effects (delete+insert), got %+v", ev.Frame.Effects)
	}
	var ed composer.Editor
	ed.Replace(composer.Draft{Text: "/res"})
	// Align editor revision to 2
	for ed.Revision() < 2 {
		ed.Changed()
	}
	for _, eff := range ev.Frame.Effects {
		if eff.Target.Revision != ed.Revision() {
			t.Fatalf("effect target revision %d != editor revision %d", eff.Target.Revision, ed.Revision())
		}
		if _, err := ed.Apply(*eff.Action); err != nil {
			t.Fatalf("apply effect %+v: %v", eff.Action, err)
		}
	}
	if ed.Text != "/resume" {
		t.Fatalf("expected editor text /resume, got %q", ed.Text)
	}

	// Receipt prunes acknowledged effects.
	_, _ = conn.Send(context.Background(), sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Receipt: &sessionapi.NativeReceipt{Effect: ev.Frame.Effects[len(ev.Frame.Effects)-1].ID},
	})
	if len(conn.effects) != 0 {
		t.Fatalf("expected effects pruned on receipt, got %+v", conn.effects)
	}
}

func TestDirectNativeProjectionCommandFileChangeAndCtrlECollapse(t *testing.T) {
	conn := &directNativeConnection{
		wire:          &nativeAppServerWire{userAgent: "mekugi/0.172.0"},
		mainThread:    "thr-main",
		items:         make(map[string][]*directItemRecord),
		itemsByID:     make(map[string]*directItemRecord),
		threadCallers: map[string]string{"thr-main": "main"},
		events:        make(chan sessionapi.NativeEvent, 16),
	}

	conn.hydrateThreadTurnsLocked("thr-main", []directWireTurn{
		{
			ID: "turn-1",
			Items: []jsontext.Value{
				jsontext.Value("{\"id\":\"cmd-1\",\"type\":\"commandExecution\",\"command\":\"/home/linuxbrew/.linuxbrew/bin/bash -lc 'printf \\\"a\\\\nb\\\\nc\\\\n\\\"'\",\"cwd\":\"/tmp/ws\",\"status\":\"completed\",\"exitCode\":0,\"durationMs\":125,\"aggregatedOutput\":\"a\\nb\\nc\\n\"}"),
				jsontext.Value("{\"id\":\"fc-1\",\"type\":\"fileChange\",\"status\":\"completed\",\"changes\":[{\"path\":\"a.go\",\"kind\":{\"type\":\"add\"},\"diff\":\"+line1\\n+line2\\n\"},{\"path\":\"b.go\",\"kind\":{\"type\":\"update\",\"move_path\":\"c.go\"},\"diff\":\"-old\\n+new\\n\"}]}"),
			},
		},
	})

	cmdRec := conn.itemsByID["cmd-1"]
	if cmdRec == nil || len(cmdRec.entry.Values) != 1 {
		t.Fatalf("expected projected commandExecution, got %+v", cmdRec)
	}
	val := cmdRec.entry.Values[0]
	if val.Lang != "bash" || strings.Contains(val.Code, "bash -lc") {
		t.Fatalf("expected unwrapped bash command, got lang=%q code=%q", val.Lang, val.Code)
	}
	if !val.Exited || val.Running || val.ExitCode != 0 || len(val.Tail) != 3 || !cmdRec.entry.OutputComplete || !cmdRec.entry.Restored {
		t.Fatalf("expected completed restored Tail projection, got val=%+v entry=%+v", val, cmdRec.entry)
	}

	fcRec := conn.itemsByID["fc-1"]
	if fcRec == nil || len(fcRec.entry.Values) != 2 {
		t.Fatalf("expected 2 fileChange values, got %+v", fcRec)
	}
	if fcRec.entry.Values[0].Verb != "Create" || fcRec.entry.Values[1].Verb != "Move" || fcRec.entry.Values[1].Label != "b.go => c.go" {
		t.Fatalf("unexpected fileChange decoding: %+v", fcRec.entry.Values)
	}

	// Verify terminalview collapses restored multi-line command output by default and ctrl+e cycles Expansion.
	var tv terminalview.View
	tv.Update(&activity.Publication{Entries: []activity.Entry{cmdRec.entry}}, terminalview.Display{})
	if tv.Expansion != 0 {
		t.Fatalf("expected default Expansion=0, got %d", tv.Expansion)
	}
	tv.ToggleExpansion()
	if tv.Expansion != 1 {
		t.Fatalf("expected Expansion=1 after first ctrl+e, got %d", tv.Expansion)
	}
	tv.ToggleExpansion()
	if tv.Expansion != 2 {
		t.Fatalf("expected Expansion=2 after second ctrl+e, got %d", tv.Expansion)
	}
	tv.ToggleExpansion()
	if tv.Expansion != 0 {
		t.Fatalf("expected Expansion=0 after third ctrl+e, got %d", tv.Expansion)
	}
}

func TestDirectNativeSkillFileCompletionsAndSettingsPickers(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "native.sock")
	srv := startMockNativeAppServer(t, socket)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatalf("openDirectNativeConnection: %v", err)
	}
	defer conn.Close()
	<-srv.ready

	drain := func() sessionapi.NativeEvent {
		var last sessionapi.NativeEvent
		for {
			select {
			case ev := <-conn.events:
				last = ev
			default:
				return last
			}
		}
	}
	_ = drain()

	// $ skill completion queries skills/list and filters enabled skills.
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Completion: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 1},
			Draft:  composer.Draft{Text: "use $rev"},
		},
	})
	if err != nil {
		t.Fatalf("$ completion: %v", err)
	}
	ev := drain()
	if ev.Frame.View.Menu == nil || len(ev.Frame.View.Menu.Choices) != 1 || ev.Frame.View.Menu.Choices[0].Name != "$review-pr" {
		t.Fatalf("expected $review-pr skill choice, got %+v", ev.Frame.View.Menu)
	}

	// @ file completion queries fuzzyFileSearch.
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Completion: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 2},
			Draft:  composer.Draft{Text: "check @main"},
		},
	})
	if err != nil {
		t.Fatalf("@ completion: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Menu == nil || len(ev.Frame.View.Menu.Choices) != 1 || ev.Frame.View.Menu.Choices[0].Name != "@cmd/main.go" {
		t.Fatalf("expected @cmd/main.go file choice, got %+v", ev.Frame.View.Menu)
	}

	// Bare /model opens MenuSettings populated from model/list (filtering hidden models).
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Command: new("/model"),
	})
	if err != nil {
		t.Fatalf("/model menu: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Menu == nil || ev.Frame.View.Menu.Kind != sessionpresentation.MenuSettings || len(ev.Frame.View.Menu.Choices) != 1 || ev.Frame.View.Menu.Choices[0].Name != "gpt-6.2-sol" {
		t.Fatalf("expected /model settings menu, got %+v", ev.Frame.View.Menu)
	}
	modelMenuID := ev.Frame.View.Menu.ID
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Menu: &sessionpresentation.MenuAction{
			Thread: "thr-main",
			Menu:   modelMenuID,
			Kind:   sessionpresentation.MenuAccept,
			Choice: completion.ChoiceID{Name: "gpt-6.2-sol"},
		},
	})
	if err != nil {
		t.Fatalf("accept /model menu: %v", err)
	}

	// Bare /tier opens MenuSettings and includes Standard even when catalog only advertises priority.
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Command: new("/tier"),
	})
	if err != nil {
		t.Fatalf("/tier menu: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Menu == nil || ev.Frame.View.Menu.Kind != sessionpresentation.MenuSettings || len(ev.Frame.View.Menu.Choices) < 2 || ev.Frame.View.Menu.Choices[0].Name != "default" {
		t.Fatalf("expected /tier settings menu with default choice first, got %+v", ev.Frame.View.Menu)
	}
	tierMenuID := ev.Frame.View.Menu.ID
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Menu: &sessionpresentation.MenuAction{
			Thread: "thr-main",
			Menu:   tierMenuID,
			Kind:   sessionpresentation.MenuAccept,
			Choice: completion.ChoiceID{Name: "default"},
		},
	})
	if err != nil {
		t.Fatalf("accept /tier menu: %v", err)
	}

	// /effort high sends "effort" (not "reasoningEffort").
	_, err = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Command: new("/effort high"),
	})
	if err != nil {
		t.Fatalf("/effort high: %v", err)
	}

	srv.mu.Lock()
	updates := slices.Clone(srv.params["thread/settings/update"])
	srv.mu.Unlock()
	if len(updates) < 3 {
		t.Fatalf("expected 3 thread/settings/update calls, got %d", len(updates))
	}
	if !strings.Contains(string(updates[1]), "\"serviceTier\":null") {
		t.Fatalf("expected serviceTier:null for standard tier, got %s", string(updates[1]))
	}
	if !strings.Contains(string(updates[2]), "\"effort\":\"high\"") {
		t.Fatalf("expected effort:high, got %s", string(updates[2]))
	}

	// Nested thread/settings/updated notification updates model, effort, and clears tier on null.
	conn.handleNotification(appserver.Message{
		Method: "thread/settings/updated",
		Params: jsontext.Value("{\"threadId\":\"thr-main\",\"threadSettings\":{\"model\":\"gpt-6.2-sol\",\"effort\":\"high\",\"serviceTier\":null}}"),
	})
	conn.mu.Lock()
	gotModel, gotEffort, gotTier := conn.model, conn.effort, conn.tier
	conn.mu.Unlock()
	if gotModel != "gpt-6.2-sol" || gotEffort != "high" || gotTier != "" {
		t.Fatalf("unexpected settings after nested update: model=%q effort=%q tier=%q", gotModel, gotEffort, gotTier)
	}

	// skills/changed invalidates cached skills so the next $ completion refreshes from skills/list.
	conn.handleNotification(appserver.Message{Method: "skills/changed"})
	conn.mu.Lock()
	loadedAfterInvalidation := conn.skillsLoaded
	conn.mu.Unlock()
	if loadedAfterInvalidation {
		t.Fatalf("expected skills/changed to invalidate cached skills")
	}

	// model/list passes threadId so custom provider catalogs are queried.
	srv.mu.Lock()
	modelListParams := slices.Clone(srv.params["model/list"])
	srv.mu.Unlock()
	if len(modelListParams) == 0 || !strings.Contains(string(modelListParams[0]), "\"threadId\":\"thr-main\"") {
		t.Fatalf("expected model/list to include threadId, got %v", modelListParams)
	}
}
