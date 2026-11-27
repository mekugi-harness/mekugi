package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"io"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

func TestDirectNativeTerminalPrediction(t *testing.T) {
	ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
	defer cancel()
	socket := filepath.Join(shortTempDir(t), "native.sock")
	server := startMockNativeAppServer(t, socket)
	requests := make(chan jsontext.Value, 8)
	server.mu.Lock()
	server.overrides["thread/start"] = func(jsontext.Value) any {
		return jsontext.Value(`{"thread":{"id":"thr-main","cwd":"/tmp/ws"},"modelProvider":"openai"}`)
	}
	server.overrides["thread/prediction/request"] = func(params jsontext.Value) any {
		requests <- params
		return jsontext.Value(`{}`)
	}
	server.mu.Unlock()
	conn, err := openDirectNativeConnection(ctx, socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	views, err := composition.NativeRenderer(false)
	if err != nil {
		t.Fatal(err)
	}
	terminal, _ := views.NewAttachment(diffrender.DarkTheme)
	t.Cleanup(func() { _ = terminal.Diff.Close() })
	terminal.Width, terminal.Height = 88, 20
	client := &nativeClient{connection: conn, renderer: nativeClientRenderer{terminal: terminal, views: views}}
	client.notifications.focused = true
	accept := func(match func(sessionapi.NativeEvent) bool) {
		t.Helper()
		client.accept(awaitDirectEvent(t, conn, match))
	}
	accept(func(event sessionapi.NativeEvent) bool { return event.Frame.View.Main.ID == "thr-main" })
	client.calls = nil

	finish := func(thread, turn string) {
		t.Helper()
		server.send(t, map[string]any{"jsonrpc": "2.0", "method": "turn/completed", "params": map[string]any{
			"threadId": thread, "turn": map[string]string{"id": turn, "status": "completed"},
		}})
	}
	request := func(turn string) {
		t.Helper()
		if client.event.Frame.Facts.PredictionTurn != turn {
			accept(func(event sessionapi.NativeEvent) bool { return event.Frame.Facts.PredictionTurn == turn })
		}
		client.acquirePrediction()
		select {
		case params := <-requests:
			var got map[string]string
			if err := json.Unmarshal(params, &got); err != nil || len(got) != 2 || got["threadId"] != "thr-main" || got["sourceTurnId"] != turn {
				t.Fatalf("request used the wrong source: %s (%v)", params, err)
			}
		case <-ctx.Done():
			t.Fatalf("no native request for %s: %v", turn, ctx.Err())
		}
	}
	notifyResult := func(thread, turn, text string) {
		t.Helper()
		server.send(t, map[string]any{"jsonrpc": "2.0", "method": "thread/prediction/updated", "params": map[string]any{
			"threadId": thread, "sourceTurnId": turn, "result": map[string]string{"type": "completed", "text": text},
		}})
	}
	result := func(turn, text string) {
		t.Helper()
		notifyResult("thr-main", turn, text)
		accept(func(event sessionapi.NativeEvent) bool {
			return event.Frame.View.Draft.Prediction == strings.TrimSpace(text)
		})
	}
	const suggestion = "Show the relevant tests"

	finish("thr-worker", "foreign")
	finish("thr-main", "turn-1")
	request("turn-1")
	notifyResult("thr-worker", "turn-1", "Wrong thread")
	result("turn-1", " "+suggestion+" ")
	rendered, err := terminal.Frame.Render(ctx, terminal.FrameInput(time.Now(), "test"))
	if err != nil {
		t.Fatal(err)
	}
	if rows := ansi.Strip(strings.Join(rendered.Rows, "\n")); !strings.Contains(rows, suggestion) {
		t.Fatalf("native suggestion %q did not reach the painted composer:\n%s", client.renderer.frame.View.Draft.Prediction, rows)
	}
	if err := terminal.Frame.Write(io.Discard, rendered, terminal.Width, terminal.Faint); err != nil {
		t.Fatal(err)
	}
	client.dispatch(terminalsession.Input{Kind: terminalsession.KeyInput, Text: "\t"})
	if got := client.editor("main").Text; got != suggestion || len(client.calls) != 0 {
		t.Fatalf("Tab should edit locally without a submission: draft=%q, calls=%d", got, len(client.calls))
	}

	client.editor("main").Take()
	finish("thr-main", "turn-2")
	request("turn-2")
	client.editor("main").InsertText("private draft")
	client.editor("main").Take()
	result("turn-2", "Arrived after typing")
	if got := client.renderer.frame.View.Draft.Prediction; got != "" {
		t.Fatalf("a changed draft revision admitted a late suggestion: %q", got)
	}

	command := func(text string) error {
		_, err := conn.Send(ctx, client.event.Target(), sessionapi.NativeOperation{Command: &text})
		return err
	}
	if err := command("/predictions off"); err == nil {
		t.Fatal("arguments to the bare toggle were accepted")
	}
	toggle := func(label string) {
		t.Helper()
		if err := command("/predictions"); err != nil {
			t.Fatal(err)
		}
		accept(func(event sessionapi.NativeEvent) bool { return event.Frame.View.Status.Feedback.Text == label })
	}
	toggle("Predictions disabled")
	client.acquirePrediction()
	if !terminal.Main.PredictionsDisabled || client.renderer.frame.View.Draft.Prediction != "" || client.event.Frame.Facts.PredictionTurn != "" {
		t.Fatal("the disabled toggle left a prediction visible or eligible")
	}
	toggle("Predictions enabled")
	request("turn-2")
	result("turn-2", "Restored on re-enable")
	if got := client.renderer.frame.View.Draft.Prediction; got != "Restored on re-enable" {
		t.Fatalf("the enabled toggle did not restore suggestions: %q", got)
	}

	finish("thr-main", "turn-3")
	request("turn-3")
	server.send(t, map[string]any{"jsonrpc": "2.0", "method": "turn/started", "params": map[string]any{
		"threadId": "thr-main", "turn": map[string]string{"id": "turn-4", "status": "inProgress"},
	}})
	accept(func(event sessionapi.NativeEvent) bool { return event.Frame.Facts.Busy })
	notifyResult("thr-main", "turn-3", "The old turn's prediction")
	finish("thr-main", "turn-4")
	request("turn-4")
	if got := client.renderer.frame.View.Draft.Prediction; got != "" {
		t.Fatalf("the new turn inherited an older suggestion: %q", got)
	}
	result("turn-4", "The new turn's prediction")

	server.send(t, map[string]any{"jsonrpc": "2.0", "method": "account/updated", "params": map[string]string{
		"authMode": "apikey",
	}})
	accept(func(event sessionapi.NativeEvent) bool {
		return event.Frame.Facts.PredictionTurn == "" && event.Frame.View.Draft.Prediction == ""
	})
	client.acquirePrediction()
	select {
	case params := <-requests:
		t.Fatalf("API-key account requested a prediction: %s", params)
	default:
	}

	server.send(t, map[string]any{"jsonrpc": "2.0", "method": "account/updated", "params": map[string]string{
		"authMode": "chatgpt",
	}})
	request("turn-4")
	result("turn-4", "Prediction after signing in")
	server.mu.Lock()
	reads := len(server.params["account/read"])
	server.mu.Unlock()
	if reads != 1 {
		t.Fatalf("read account %d times for multiple predictions, want once", reads)
	}
}
