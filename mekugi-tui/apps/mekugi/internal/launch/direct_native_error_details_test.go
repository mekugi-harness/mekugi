package launch

import (
	"encoding/base64"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func TestUISnapshotDirectNativeErrorDetails(t *testing.T) {
	const firstError = "Earlier provider failure"
	const tail = "forced cleanup is not permitted"
	diagnostic := "CreateProcess: " + strings.Repeat("command ", 9000) + "\nrejected: " + tail + "\n    at cell:17"
	full := "exec script failed: " + diagnostic
	socket := filepath.Join(shortTempDir(t), "errors.sock")
	server := startMockNativeAppServer(t, socket)
	server.mu.Lock()
	server.overrides["thread/resume"] = func(jsontext.Value) any {
		return map[string]any{"thread": map[string]any{
			"id": "thr-main", "cwd": "/tmp/ws", "name": "Error details",
			"turns": []any{
				map[string]any{"id": "previous", "status": "failed", "error": map[string]any{"message": firstError}},
				map[string]any{"id": "recovered", "status": "completed", "items": []any{
					map[string]any{"type": "functionCallOutput", "id": "code-mode-error:cell",
						"name": "exec", "namespace": "functions", "output": diagnostic},
					map[string]any{"type": "agentMessage", "id": "answer", "text": "Recovered in the same turn."},
				}},
			},
		}}
	}
	server.mu.Unlock()
	conn, err := openDirectNativeConnection(t.Context(), socket, "/tmp/ws", nil, nil, "thr-main", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.Main != nil })
	now := time.Date(2026, 10, 2, 12, 40, 21, 0, time.Local)
	conn.mu.Lock()
	for _, rec := range conn.items["thr-main"] {
		rec.entry.Observed, rec.entry.Completed = now, now
	}
	conn.publishLocked()
	pinnedRevision := conn.revision
	conn.mu.Unlock()
	ev := awaitDirectEvent(t, conn, func(next sessionapi.NativeEvent) bool {
		return next.Frame.View.Version.Revision >= pinnedRevision
	})

	views, err := composition.NativeRenderer(false)
	if err != nil {
		t.Fatal(err)
	}
	u, _ := views.NewAttachment(diffrender.DarkTheme)
	u.Width, u.Height = 88, 21
	client := &nativeClient{connection: conn, event: ev, renderer: nativeClientRenderer{terminal: u, views: views}}
	client.editor("main").InsertText("preserve draft")
	paint := func() string {
		t.Helper()
		client.refreshView(now)
		input := u.FrameInput(now, "test")
		rendered, err := u.Frame.Render(t.Context(), input)
		if err != nil {
			t.Fatal(err)
		}
		if err := u.Frame.Write(io.Discard, rendered, u.Width, u.Faint); err != nil {
			t.Fatal(err)
		}
		return ansi.Strip(strings.Join(rendered.Rows, "\n"))
	}
	dispatch := func(kind terminalsession.InputKind, value string) {
		t.Helper()
		client.dispatch(terminalsession.Input{Kind: kind, Text: value})
		for _, call := range client.calls {
			if _, err := conn.Send(t.Context(), call.target, call.operation); err != nil {
				t.Fatal(err)
			}
		}
		client.calls = nil
	}
	adoptFrame := func(match func(sessionapi.NativeEvent) bool) {
		t.Helper()
		client.event = awaitDirectEvent(t, conn, match)
		paint()
	}
	frame := paint()
	if strings.Contains(frame, tail) || !strings.Contains(frame, "details · ctrl+b !") {
		t.Fatalf("transcript did not bound the full error with its Details hint:\n%s", frame)
	}
	dispatch(terminalsession.KeyInput, "\x02")
	dispatch(terminalsession.KeyInput, "!")
	adoptFrame(func(ev sessionapi.NativeEvent) bool { return ev.Frame.Details != nil })
	if u.Dialog.Page != 1 || len(u.Dialog.Pages) != 2 || u.Dialog.Document.Text != full {
		t.Fatal("error history did not select the newest complete diagnostic")
	}
	dispatch(terminalsession.KeyInput, "\x1b[Z")
	paint()
	if u.Dialog.Document.Text != firstError {
		t.Fatalf("previous error page = %q", u.Dialog.Document.Text)
	}
	dispatch(terminalsession.KeyInput, "\x1b")
	adoptFrame(func(ev sessionapi.NativeEvent) bool { return ev.Frame.Details == nil })

	// The row disclosure targets only that error, without changing the editor.
	x, y := 0, 0
	for row, snippet := range u.Transcript.FeedSnippets {
		if block, ok := u.Transcript.SnippetBlock(snippet); ok && block.Kind == "error" && block.Body == full {
			x = u.Layout.Main.X + u.Transcript.FeedLeft
			y = u.Layout.Main.Y + u.Transcript.FeedTop + row
			break
		}
	}
	if y == 0 {
		t.Fatal("painted script error has no pointer target")
	}
	dispatch(terminalsession.MouseInput, fmt.Sprintf("\x1b[<0;%d;%dM", x, y))
	dispatch(terminalsession.MouseInput, fmt.Sprintf("\x1b[<0;%d;%dm", x, y))
	adoptFrame(func(ev sessionapi.NativeEvent) bool { return ev.Frame.Details != nil })
	if len(u.Dialog.Pages) != 1 || client.editor("main").Text != "preserve draft" {
		t.Fatal("click did not open only the selected error while preserving the draft")
	}
	for _, key := range []string{"/", tail, "\r"} {
		dispatch(terminalsession.KeyInput, key)
	}
	if screen := paint(); !strings.Contains(screen, tail) {
		t.Fatalf("search cannot reach the suffix beyond visual wrapping:\n%s", screen)
	}
	dispatch(terminalsession.KeyInput, "y")
	if u.Frame.Clipboard != "\x1b]52;c;"+base64.StdEncoding.EncodeToString([]byte(full))+"\x07" {
		t.Fatal("copy lost part of the retained diagnostic")
	}
	uisnapshot.Assert(t, "testdata/snapshots/direct-native-error-details.txt", frame)
}

func TestDirectNativeErrorHistoryScopesLiveNotifications(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "errors.sock")
	server := startMockNativeAppServer(t, socket)
	server.mu.Lock()
	const childError = "Delegate failed independently"
	const childScript = "Delegate script diagnostic"
	scriptFailure := func(diagnostic string) map[string]any {
		return map[string]any{"type": "functionCallOutput", "id": "code-mode-error:shared",
			"name": "exec", "namespace": "functions", "output": diagnostic}
	}
	server.overrides["thread/read"] = func(jsontext.Value) any {
		return map[string]any{"thread": map[string]any{"id": "thr-worker", "turns": []any{
			map[string]any{"id": "live", "status": "completed", "items": []any{scriptFailure(childScript)}},
			map[string]any{"id": "other", "status": "failed", "error": map[string]any{"message": childError}},
		}}}
	}
	server.mu.Unlock()
	conn, err := openDirectNativeConnection(t.Context(), socket, "/tmp/ws", nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	sendNotification := func(method string, params map[string]any) {
		t.Helper()
		server.send(t, map[string]any{"jsonrpc": "2.0", "method": method, "params": params})
	}
	failure := map[string]any{"message": "Provider disconnected", "additionalDetails": "Additional decisive text"}
	sendNotification("error", map[string]any{"threadId": "thr-main", "turnId": "live", "willRetry": true, "error": map[string]any{"message": "Temporary connection problem"}})
	sendNotification("error", map[string]any{"threadId": "thr-main", "turnId": "live", "willRetry": false, "error": failure})
	sendNotification("turn/completed", map[string]any{"threadId": "thr-main", "turn": map[string]any{"id": "live", "status": "failed", "error": failure}})
	const full = "Provider disconnected · Additional decisive text"
	ev := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Status.Feedback.Text != "" })
	if entries := ev.Frame.Main.Entries; len(entries) != 1 || entries[0].Kind != "error" || entries[0].Values[0].Body != full {
		t.Fatalf("live terminal history did not merge the completion and keep its full error: %+v", entries)
	}
	const mainScript = "Main script diagnostic"
	sendNotification("item/completed", map[string]any{"threadId": "thr-main", "turnId": "live", "item": scriptFailure(mainScript)})
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return len(ev.Frame.Main.Entries) == 2 })
	send := func(operation sessionapi.NativeOperation) {
		t.Helper()
		if _, err := conn.Send(t.Context(), sessionapi.NativeTarget{}, operation); err != nil {
			t.Fatal(err)
		}
	}
	send(sessionapi.NativeOperation{Display: &sessionpresentation.DisplayIntent{Thread: sessionpresentation.Thread{ID: "thr-worker", Caller: "/root/worker"}}})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Displayed.ID == "thr-worker" && ev.Frame.Agents != nil && len(ev.Frame.Agents.Entries) == 2
	})
	if entries := ev.Frame.Main.Entries; len(entries) != 2 || entries[1].Values[0].Body != "exec script failed: "+mainScript {
		t.Fatalf("delegate tool call ID replaced Main's script error: %+v", entries)
	}
	send(sessionapi.NativeOperation{Details: &sessionapi.NativeDetailRequest{
		Kind: sessionapi.NativeErrorDetails, Agent: true, Displayed: ev.Frame.View.Displayed,
	}})
	ev = awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.Details != nil })
	if d := ev.Frame.Details; d.Source.ID != "thr-worker" || !d.Agent || len(d.View.Pages) != 2 ||
		d.View.Pages[0].Body != "exec script failed: "+childScript || d.View.Pages[1].Body != childError {
		t.Fatalf("delegate history disclosed another thread's feedback: %+v", d)
	}
	send(sessionapi.NativeOperation{Notice: &sessionapi.NativeNoticeAction{Version: ev.Frame.View.Version, Feedback: ev.Frame.View.Status.FeedbackRevision}})
	ev = awaitDirectEvent(t, conn, func(next sessionapi.NativeEvent) bool {
		return next.Frame.Details != nil && next.Frame.Details.View.ID != ev.Frame.Details.View.ID
	})
	if pages := ev.Frame.Details.View.Pages; len(pages) != 1 || pages[0].Body != full {
		t.Fatalf("current notice no longer opens its own diagnostic: %+v", pages)
	}
}
