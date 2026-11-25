package launch

import (
	"context"
	"fmt"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/vt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

func TestNativeClientArrowAndMouseSwitchRetainLargeFamilyOwners(t *testing.T) {
	u, _ := newAppServerTestUI()
	workspace := t.TempDir()

	threads := []string{"main", "child-a", "child-b"}
	callers := map[string]string{
		"main":    "/root",
		"child-a": "/root/alpha",
		"child-b": "/root/beta",
	}
	roster := &activity.Publication{Roster: make([]activity.AgentRow, 0, len(threads))}
	for _, id := range threads {
		name := callers[id]
		roster.Roster = append(roster.Roster, activity.AgentRow{
			Agent: activity.Agent{AgentFacts: activity.AgentFacts{Name: name, Thread: id}},
			Depth: strings.Count(name, "/") - 1,
		})
	}
	family := &sessionapi.Family{Root: "main", Revision: 5, Threads: threads, Agents: roster}
	review := &changes.ReviewView{
		Workspace: workspace,
		Revision:  9,
		Publication: changes.Publication{
			Files: []changes.File{{
				Path: "pkg/app.go",
				Chunks: []changes.Chunk{{
					Workspace: workspace,
					Key:       "chunk-1",
					Caller:    "/root/alpha",
					Review:    diffmodel.ReviewFile{Diff: strings.Repeat("+saved diff line\n", 2048)},
				}},
			}},
		},
	}

	launcher := &nativeReattachLauncher{run: func(ctx context.Context, start sessionapi.Start, api *sessionapi.Server) error {
		id := start.Resume
		thread := sessionpresentation.Thread{ID: id, Caller: callers[id]}
		view := sessionpresentation.View{
			Version: sessionpresentation.Version{Attachment: 1, Revision: 1},
			Main:    thread, Displayed: thread, Input: thread,
			Info: sessionpresentation.MainInfo{Workspace: workspace, Caller: thread.Caller},
		}
		if err := api.SetSummary(sessionapi.Summary{Thread: id, Workspace: workspace, Ready: true}); err != nil {
			return err
		}
		mainPub := &activity.Publication{
			Entries: []activity.Entry{{Seq: 1, Agent: callers[id], Kind: "user", Text: "transcript for " + id, OutputComplete: true, Revision: 1}},
			LastSeq: 1,
		}
		for {
			select {
			case <-ctx.Done():
				return ctx.Err()
			case request := <-api.Native().Requests():
				frame := sessionapi.NativeFrame{
					View:   view,
					Main:   mainPub,
					Family: family,
					Review: review,
				}
				if err := api.Native().Publish(request.Attachment, frame); err != nil {
					return err
				}
				request.Reply <- ""
			}
		}
	}}

	hub := sessionapi.NewHub(t.Context(), launcher)
	t.Cleanup(hub.Close)
	server := httptest.NewServer(sessionapi.Versioned(hub.Handler()))
	t.Cleanup(server.Close)
	rpc, err := sessionapi.OpenRPC(t.Context(), server.Client(), server.URL+sessionapi.RPCPath, "")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = rpc.Close() })
	for _, id := range threads {
		serverRPCCall(t, rpc, "session/start", "", sessionapi.Start{Workspace: workspace, Model: "fixture", Resume: id})
	}

	connection, err := sessionapi.OpenNative(t.Context(), server.Client(), server.URL, "main")
	if err != nil {
		t.Fatal(err)
	}
	terminal, _ := u.terminalViews.NewAttachment(diffrender.DarkTheme)
	terminal.Width, terminal.Height = 120, 36
	c := &nativeClient{connection: connection, renderer: nativeClientRenderer{terminal: terminal, views: u.terminalViews}}
	r := &nativeClientRuntime{
		ctx: t.Context(), active: c,
		messages:   make(chan nativeClientMessage),
		results:    make(chan *nativeClientCall, nativeCallLanes),
		reattached: make(chan nativeClientReattached, 1),
		editors:    make(map[[2]string]*composer.Editor),
		images:     make(map[string]bool),
	}
	r.start(c, nil)
	t.Cleanup(r.close)
	for c.event.Attachment == "" {
		nativeSchedulingStep(t, r)
	}

	screen := vt.NewEmulator(120, 36)
	t.Cleanup(func() { _ = screen.Close() })
	paint := func() string {
		t.Helper()
		if _, err := r.active.renderer.render(t.Context(), screen, time.Now(), "dev"); err != nil {
			t.Fatal(err)
		}
		return screen.String()
	}

	// Open roster sheet (^B 3) and paint so hits/layout are populated.
	now := time.Now()
	for _, b := range []byte("\x023") {
		r.decode(b, now)
	}
	r.active.drainInput()
	paint()
	initialReview := r.active.event.Frame.Review
	initialFamily := r.active.event.Frame.Family

	// 1. Arrow Down switches to child-a while retaining Review and Family pointers.
	for _, b := range []byte("\x1b[B") {
		r.decode(b, now)
	}
	r.active.drainInput()
	for r.active.event.Frame.View.Main.ID != "child-a" {
		nativeSchedulingStep(t, r)
	}
	if r.active.event.Frame.Review != initialReview || r.active.event.Frame.Family != initialFamily {
		t.Fatal("arrow switch did not retain Review and Family across attachment")
	}
	if rendered := paint(); !strings.Contains(rendered, "alpha") {
		t.Fatal("arrow switch first paint did not render child-a view")
	}

	// 2. Mouse click on child-b row switches to child-b while retaining Review and Family pointers.
	rosterRect := r.active.renderer.terminal.Layout.Roster
	click := fmt.Sprintf("\x1b[<0;%d;%dM", rosterRect.X+5, rosterRect.Y+3)
	for i := range len(click) {
		r.decode(click[i], now)
	}
	r.active.drainInput()
	for r.active.event.Frame.View.Main.ID != "child-b" {
		nativeSchedulingStep(t, r)
	}
	if r.active.event.Frame.Review != initialReview || r.active.event.Frame.Family != initialFamily {
		t.Fatal("mouse switch did not retain Review and Family across attachment")
	}
	if rendered := paint(); !strings.Contains(rendered, "beta") {
		t.Fatal("mouse switch first paint did not render child-b view")
	}
	// Close the roster sheet (Escape) and verify child-b's transcript renders in full pane.
	r.decode(0x1b, now)
	r.active.drainInput()
	if r.active.renderer.terminal.Decoder.Escape(now.Add(time.Second)) {
		r.active.queueInput(terminalsession.Input{Kind: terminalsession.KeyInput, Text: "\x1b"})
		r.active.drainInput()
	}
	if rendered := paint(); !strings.Contains(rendered, "transcript for child-b") {
		t.Fatalf("focus=%d rendered:\n%s", r.active.renderer.terminal.Focus, rendered)
	}
}

func TestReadRestoredRolloutSkipsOrdinaryFunctionCallOutput(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "rollout.jsonl")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	fmt.Fprintf(f, "{\"timestamp\":\"2026-11-24T12:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"child-1\"}}\n")
	fmt.Fprintf(f, "{\"timestamp\":\"2026-11-24T12:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_started\",\"turn_id\":\"turn-1\"}}\n")
	fmt.Fprintf(f, "{\"timestamp\":\"2026-11-24T12:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"c1\",\"output\":%q}}\n", strings.Repeat("ordinary output ", 512))
	fmt.Fprintf(f, "{\"timestamp\":\"2026-11-24T12:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"item_completed\",\"turn_id\":\"turn-1\",\"item\":{\"id\":\"item-1\"}}}\n")
	_ = f.Close()

	got := readRestoredRollout(binding.ThreadInfo{ID: "child-1", Path: path, Cwd: dir}, "/root/worker")
	if _, ok := got.itemAt["item-1"]; !ok {
		t.Fatal("rollout reader missed item_completed after skipped function_call_output")
	}
}

