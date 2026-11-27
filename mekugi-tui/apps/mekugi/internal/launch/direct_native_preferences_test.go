package launch

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

func TestDirectNativeRestoresTerminalPreferencesOnColdResume(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	f := newNativeParitySnapshot(t)
	f.server.mu.Lock()
	f.server.overrides["thread/resume"] = func(params jsontext.Value) any {
		var input struct {
			ThreadID string `json:"threadId"`
		}
		if err := json.Unmarshal(params, &input); err != nil {
			return mockRPCError(err.Error())
		}
		return map[string]any{"thread": map[string]string{"id": input.ThreadID, "cwd": "/tmp/ws"}}
	}
	f.server.mu.Unlock()

	original := f.await(func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Main.ID == "thr-main" })
	f.client.accept(original)
	before := f.client.renderer.terminal
	before.Shell.RestorePreferences(terminalsession.Preferences{
		Focus: 1, Split: 67, DiffOpen: true, NavigatorColumns: 20,
		InterruptLocked: true, PredictionsDisabled: true,
	}, before.Main)
	want := before.Shell.Preferences(before.Main)
	f.client.stagePreferences(false)
	var staged *nativeClientCall
	for _, call := range f.client.calls {
		if call.operation.Preferences != nil {
			staged = call
		}
	}
	if staged == nil {
		t.Fatal("a fresh attachment did not begin saving changed layout")
	}

	// If the renderer's queued write reaches Send after another thread opens,
	// it must still save the outgoing thread's layout, not the replacement's.
	const replacement = "c652ea78-b14a-40fe-8a65-0e37d6a17ddf"
	f.send(sessionapi.NativeOperation{Command: new("/resume " + replacement)})
	f.await(func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Main.ID == replacement })
	if _, err := f.conn.Send(f.ctx, staged.target, staged.operation); err != nil {
		t.Fatal(err)
	}
	if err := f.conn.Close(); err != nil {
		t.Fatal(err)
	}

	for _, thread := range []string{"thr-main", replacement} {
		conn, err := openDirectNativeConnection(f.ctx, f.conn.opts.socket, "/tmp/ws", nil, nil, thread, "")
		if err != nil {
			t.Fatal(err)
		}
		ev := awaitDirectEvent(t, conn, func(ev sessionapi.NativeEvent) bool { return ev.Frame.View.Main.ID == thread })
		if err := conn.Close(); err != nil {
			t.Fatal(err)
		}
		views := f.client.renderer.views
		terminal, _ := views.NewAttachment(diffrender.DarkTheme)
		client := &nativeClient{renderer: nativeClientRenderer{views: views, terminal: terminal}}
		client.accept(ev)
		have := terminal.Shell.Preferences(terminal.Main)
		if thread == "thr-main" {
			if have != want {
				t.Fatalf("cold layout = %+v, want %+v", have, want)
			}
		} else if ev.Frame.Facts.Preferences != nil {
			t.Fatalf("outgoing layout was saved for replacement thread: %+v", have)
		}
	}
}
