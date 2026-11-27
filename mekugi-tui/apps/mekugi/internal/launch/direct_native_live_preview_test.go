package launch

import (
	"io"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

func TestDirectNativeLivePreviewFollowsDisplayedThread(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "native.sock")
	server := startMockNativeAppServer(t, socket)
	ctx := t.Context()
	conn, err := openDirectNativeConnection(ctx, socket, t.TempDir(), nil, nil, "", "")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { conn.Close() })
	awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Main.ID == "thr-main" && ev.Frame.Family != nil
	})

	views, err := composition.NativeRenderer(false)
	if err != nil {
		t.Fatal(err)
	}
	terminal, _ := views.NewAttachment(diffrender.DarkTheme)
	terminal.Width, terminal.Height = 120, 40
	renderer := nativeClientRenderer{terminal: terminal, views: views}
	now := time.Date(2026, 11, 26, 12, 0, 0, 0, time.UTC)
	paint := func(ev sessionapi.NativeEvent) string {
		t.Helper()
		renderer.update(ev.Frame, conversation.DraftView{}, conversation.PromptDraftView{}, now)
		now = now.Add(time.Second)
		rendered, err := renderer.render(ctx, io.Discard, now, "0.172.0")
		if err != nil {
			t.Fatal(err)
		}
		if len(rendered.Demands) != 0 {
			t.Fatalf("unexpected output demands: %v", rendered.Demands)
		}
		dock := terminal.Shell.Layout.Live
		return ansi.Strip(strings.Join(rendered.Rows[dock.Y:dock.Y+dock.H], "\n"))
	}
	startEdit := func(thread, path, text string) sessionapi.NativeEvent {
		t.Helper()
		server.send(t, map[string]any{
			"jsonrpc": "2.0", "method": "item/started",
			"params": map[string]any{
				"threadId": thread, "turnId": "turn-" + thread,
				"item": map[string]any{
					"type": "fileChange", "id": "edit-" + thread, "status": "inProgress",
					"changes": []map[string]any{{
						"path": path, "kind": map[string]string{"type": "update"},
						"diff": "@@ -1 +1 @@\n-old\n+" + text + "\n",
					}},
				},
			},
		})
		return awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
			for _, edit := range ev.Frame.Live {
				if edit.Thread == thread {
					return true
				}
			}
			return false
		})
	}
	display := func(ev sessionapi.NativeEvent, id, caller string) sessionapi.NativeEvent {
		t.Helper()
		if _, err := conn.Send(ctx, ev.Target(), sessionapi.NativeOperation{
			Display: &sessionpresentation.DisplayIntent{Thread: sessionpresentation.Thread{ID: id, Caller: caller}},
		}); err != nil {
			t.Fatal(err)
		}
		return awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool {
			return ev.Frame.View.Displayed.ID == id
		})
	}

	main := startEdit("thr-main", "main.go", "main preview hunk")
	if shown := paint(main); !strings.Contains(shown, "main preview hunk") {
		t.Fatalf("Main live diff did not survive resync:\n%s", shown)
	}
	worker := display(main, "thr-worker", "/root/worker")
	if shown := paint(worker); strings.Contains(shown, "main preview hunk") || terminal.Live.Live() != 0 {
		t.Fatalf("selected worker retained Main's live diff:\n%s", shown)
	}
	worker = startEdit("thr-worker", "worker.go", "worker preview hunk")
	if shown := paint(worker); !strings.Contains(shown, "worker preview hunk") {
		t.Fatalf("selected worker's live diff did not survive resync:\n%s", shown)
	}
	if shown := paint(display(worker, "thr-main", "/root")); terminal.Live.Live() != 2 {
		t.Fatalf("Main did not retain both admitted live diffs:\n%s", shown)
	}
}
