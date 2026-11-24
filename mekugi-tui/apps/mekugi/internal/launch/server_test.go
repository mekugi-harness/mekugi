package launch

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	jsonv2 "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandargs"
)

func TestRunServerLifecycle(t *testing.T) {
	if os.Getenv("MEKUGI_TEST_SERVER_CLI_HOST") == "1" {
		serveServerCLIHost(t)
		os.Exit(0)
	}
	for _, key := range []string{"XDG_CONFIG_HOME", "XDG_STATE_HOME", "CODEX_HOME", "MEKUGI_RUNTIME_DIR", "TMPDIR"} {
		t.Setenv(key, t.TempDir())
	}
	logPath := filepath.Join(t.TempDir(), "host.jsonl")
	t.Setenv("MEKUGI_TEST_SERVER_CLI_LOG", logPath)
	t.Cleanup(func() {
		if t.Failed() {
			body, _ := os.ReadFile(logPath)
			t.Logf("host requests: %s", body)
		}
	})
	bin := t.TempDir()
	// Catalog discovery is a separate command, not an app-server protocol peer.
	script := "#!/bin/sh\nif [ \"$1\" = debug ]; then exit 1; fi\nMEKUGI_TEST_SERVER_CLI_HOST=1; export MEKUGI_TEST_SERVER_CLI_HOST\nexec " + commandargs.Quote(os.Args[0]) + " -test.run=^TestRunServerLifecycle$ -- \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "mekugi-app-server"), []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	ctx, cancel := context.WithCancel(t.Context())
	socketDirectory := t.TempDir()
	if err := os.Chmod(socketDirectory, 0700); err != nil {
		t.Fatal(err)
	}
	socket := filepath.Join(socketDirectory, "s")
	var diagnostic bytes.Buffer
	done := make(chan int, 1)
	go func() { done <- RunServer(ctx, []string{"--socket", socket}, &diagnostic) }()
	joined := false
	defer func() {
		cancel()
		if !joined {
			select {
			case <-done:
			case <-time.After(10 * time.Second):
				t.Error("server did not join owned runtimes")
			}
		}
	}()
	transport := sessionapi.ControlTransport(socket)
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 10 * time.Second}
	request := func(method, path, body string) *http.Response {
		t.Helper()
		req, err := http.NewRequestWithContext(t.Context(), method, "http://mekugi"+path, strings.NewReader(body))
		if err != nil {
			t.Fatal(err)
		}
		req.Header.Set(sessionapi.ProtocolHeader, sessionapi.ProtocolVersion)
		req.Header.Set("Content-Type", "application/json")
		response, err := client.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { response.Body.Close() })
		return response
	}
	serverCLIAwait(t, func() bool {
		select {
		case code := <-done:
			joined = true
			t.Fatalf("server stopped before readiness: %d: %s", code, &diagnostic)
		default:
		}
		info, err := os.Stat(socket)
		return err == nil && info.Mode()&os.ModeSocket != 0
	})
	version := request("GET", "/api/version", "")
	if version.StatusCode != http.StatusOK || version.Header.Get(sessionapi.ProtocolHeader) != sessionapi.ProtocolVersion {
		t.Fatalf("version response: %s, %v", version.Status, version.Header)
	}
	version.Body.Close()
	rpc, err := sessionapi.OpenRPC(t.Context(), client, "http://mekugi"+sessionapi.RPCPath, "")
	if err != nil {
		t.Fatal(err)
	}
	defer rpc.Close()
	workspace := t.TempDir()
	create := func(text string) string {
		data := serverRPCCall(t, rpc, "session/start", "", sessionapi.Start{Workspace: workspace, Model: "codex"})
		var created struct {
			ID string `json:"id"`
		}
		if err := jsonv2.Unmarshal(data, &created); err != nil || created.ID != "main" {
			t.Fatalf("logical identity %q: %v", created.ID, err)
		}
		serverRPCCall(t, rpc, "session/input", created.ID, sessionapi.InputRequest{Text: text})
		return created.ID
	}
	thread := create("initial message")
	events := serverRPCSubscribe(t, rpc, thread)
	serverCLIEvent(t, events, "", "offline host response 1")
	events.Close()
	events = serverRPCSubscribe(t, rpc, thread)
	serverCLIEvent(t, events, "snapshot", "offline host response 1")
	serverRPCCall(t, rpc, "session/input", thread, sessionapi.InputRequest{Text: "later message"})
	serverCLIEvent(t, events, "patch", "offline host response 2")
	events.Close()
	serverRPCCall(t, rpc, "session/input", thread, sessionapi.InputRequest{Text: "after disconnect"})
	serverCLIAwait(t, func() bool {
		body, _ := os.ReadFile(logPath)
		return strings.Contains(string(body), "initial message") && strings.Contains(string(body), "later message") && strings.Contains(string(body), "after disconnect")
	})
	serverRPCCall(t, rpc, "session/end", thread, nil)
	serverCLIAwait(t, func() bool {
		err := rpc.Call(t.Context(), "session/read", sessionapi.RPCParams{Thread: thread}, nil)
		failure, ok := errors.AsType[*sessionapi.RPCError](err)
		return ok && failure.Data != nil && failure.Data.Status == 404
	})
	// Ending an invocation releases its private resources, but the live server
	// retains exactly one shared worker image until shutdown.
	entries, err := os.ReadDir(os.Getenv("MEKUGI_RUNTIME_DIR"))
	if err != nil || len(entries) != 1 || !entries[0].IsDir() || !strings.HasPrefix(entries[0].Name(), "mekugi-worker-") {
		t.Fatalf("expected only the server-owned worker image: %v, %v", entries, err)
	}
	workerDirectory := filepath.Join(os.Getenv("MEKUGI_RUNTIME_DIR"), entries[0].Name())
	workers, err := os.ReadDir(workerDirectory)
	if err != nil || len(workers) != 1 || workers[0].Name() != ".mekugi-worker" {
		t.Fatalf("unexpected server-owned worker image contents: %v, %v", workers, err)
	}
	worker, err := workers[0].Info()
	if err != nil || !worker.Mode().IsRegular() || worker.Mode().Perm()&0111 == 0 {
		t.Fatalf("server-owned worker image is not executable: %v, %v", worker, err)
	}
	if entries, err := os.ReadDir(os.Getenv("TMPDIR")); err != nil || len(entries) != 0 {
		t.Errorf("owned TMPDIR resources survived: %v, %v", entries, err)
	}
	// A new live runtime proves end did not stop the server. Shutdown must join it.
	create("server shutdown message")
	cancel()
	select {
	case code := <-done:
		joined = true
		if code != 0 {
			t.Fatalf("server exit = %d: %s", code, &diagnostic)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("shutdown did not join its live host")
	}
	serverCLIResourcesEmpty(t)
	if _, err := os.Lstat(socket); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("owned socket survived shutdown: %v", err)
	}
}

func TestRunServerArguments(t *testing.T) {
	for _, test := range []struct {
		name string
		args []string
		code int
	}{
		{"help", []string{"--help"}, 0},
		{"unknown flag", []string{"--unknown"}, 2},
		{"missing socket operand", []string{"--socket"}, 2},
		{"positional argument", []string{"unexpected"}, 2},
		{"relative socket", []string{"--socket", "relative.sock"}, 1},
	} {
		t.Run(test.name, func(t *testing.T) {
			var diagnostic bytes.Buffer
			if code := RunServer(t.Context(), test.args, &diagnostic); code != test.code || diagnostic.Len() == 0 {
				t.Fatalf("exit = %d, diagnostic = %q", code, &diagnostic)
			}
		})
	}
}

func serverCLIAwait(t *testing.T, predicate func() bool) {
	t.Helper()
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	tick := time.NewTicker(10 * time.Millisecond)
	defer tick.Stop()
	for !predicate() {
		select {
		case <-deadline.C:
			t.Fatal("timed out waiting for server lifecycle state")
		case <-tick.C:
		}
	}
}

func serverRPCCall(t *testing.T, rpc *sessionapi.RPCClient, method, thread string, body any) jsontext.Value {
	t.Helper()
	data, err := jsonv2.Marshal(body)
	if err != nil {
		t.Fatal(err)
	}
	if body == nil {
		data = nil
	}
	var result jsontext.Value
	if err := rpc.Call(t.Context(), method, sessionapi.RPCParams{Thread: thread, Body: data}, &result); err != nil {
		t.Fatalf("%s: %v", method, err)
	}
	return result
}

func serverRPCSubscribe(t *testing.T, rpc *sessionapi.RPCClient, thread string) *sessionapi.RPCSubscription {
	t.Helper()
	events, err := rpc.Subscribe(t.Context(), "session/subscribe", sessionapi.RPCParams{Thread: thread})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(events.Close)
	return events
}

func serverCLIEvent(t *testing.T, subscription *sessionapi.RPCSubscription, name, text string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	var seen []string
	for {
		event, err := subscription.Next(ctx)
		if err != nil {
			t.Fatalf("stream ended before %s containing %q (events %v): %v", name, text, seen, err)
		}
		seen = append(seen, event.Kind)
		if (name == "" || event.Kind == name) && strings.Contains(string(event.Data), text) {
			var payload map[string]jsontext.Value
			if err := jsonv2.Unmarshal(event.Data, &payload); err != nil || len(payload) == 0 {
				t.Fatalf("invalid %s event: %s (%v)", name, event.Data, err)
			}
			if name == "snapshot" {
				var session struct {
					Thread string `json:"thread"`
				}
				if err := jsonv2.Unmarshal(payload["session"], &session); err != nil || session.Thread != "main" {
					t.Fatalf("snapshot identity %q: %v", session.Thread, err)
				}
			}
			return
		}
	}
}

func serverCLIResourcesEmpty(t *testing.T) {
	t.Helper()
	for _, key := range []string{"MEKUGI_RUNTIME_DIR", "TMPDIR"} {
		entries, err := os.ReadDir(os.Getenv(key))
		if err != nil || len(entries) != 0 {
			t.Errorf("owned %s resources survived: %v, %v", key, entries, err)
		}
	}
}

func serveServerCLIHost(t *testing.T) {
	t.Helper()
	log, err := os.OpenFile(os.Getenv("MEKUGI_TEST_SERVER_CLI_LOG"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		t.Fatal(err)
	}
	defer log.Close()
	decoder, encoder := jsontext.NewDecoder(os.Stdin), jsontext.NewEncoder(os.Stdout)
	turn, input := 0, 0
	for {
		var request struct {
			ID     jsontext.Value `json:"id"`
			Method string         `json:"method"`
			Params jsontext.Value `json:"params"`
		}
		if err := jsonv2.UnmarshalDecode(decoder, &request); err != nil {
			if !errors.Is(err, io.EOF) {
				t.Fatal(err)
			}
			return
		}
		if len(request.ID) == 0 {
			continue
		}
		if _, err := fmt.Fprintf(log, "%s %s\n", request.Method, request.Params); err != nil {
			t.Fatal(err)
		}
		result := map[string]any{}
		switch request.Method {
		case "config/read":
			result["layers"] = []any{}
		case "thread/start":
			if gate := os.Getenv("MEKUGI_TEST_SERVER_CLI_START_GATE"); gate != "" {
				serverCLIAwait(t, func() bool { _, err := os.Stat(gate); return err == nil })
			}
			workspace, err := os.Getwd()
			if err != nil {
				t.Fatal(err)
			}
			result["thread"] = map[string]any{"id": "main", "cwd": workspace}
		case "turn/start":
			turn++
			result["turn"] = map[string]any{"id": fmt.Sprintf("turn-%d", turn)}
		case "turn/steer":
			result["turnId"] = fmt.Sprintf("turn-%d", turn)
		}
		if err := jsonv2.MarshalEncode(encoder, map[string]any{"id": request.ID, "result": result}); err != nil {
			t.Fatal(err)
		}
		if request.Method == "turn/start" || request.Method == "turn/steer" {
			id := fmt.Sprintf("turn-%d", turn)
			input++
			item := fmt.Sprintf("answer-%d", input)
			text := fmt.Sprintf("offline host response %d", input)
			for _, event := range []map[string]any{
				{"method": "turn/started", "params": map[string]any{"threadId": "main", "turn": map[string]any{"id": id}}},
				{"method": "item/started", "params": map[string]any{"threadId": "main", "turnId": id, "item": map[string]any{"id": item, "type": "agentMessage", "text": ""}}},
				{"method": "item/agentMessage/delta", "params": map[string]any{"threadId": "main", "turnId": id, "itemId": item, "delta": text}},
				{"method": "item/completed", "params": map[string]any{"threadId": "main", "turnId": id, "item": map[string]any{"id": item, "type": "agentMessage", "text": text}}},
			} {
				if err := jsonv2.MarshalEncode(encoder, event); err != nil {
					t.Fatal(err)
				}
			}
		}
	}
}
