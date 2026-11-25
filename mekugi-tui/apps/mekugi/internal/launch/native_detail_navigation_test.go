package launch

import (
	"bytes"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/vt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

func TestNativeDetailUnknownSkillCallerIsStale(t *testing.T) {
	u, _ := newAppServerTestUI()
	u.startSession("main", t.TempDir())
	u.attached.session.RegisterThread(observation.Thread{ID: "child", Path: "/root/child"})
	sessionpresentation.ApplyActivity(u.Thread, u.attached.source, u.attached.childSource, nil, u.attached.session.Roster())
	for _, roster := range []bool{false, true} {
		c := &nativeClient{event: sessionapi.NativeEvent{Frame: sessionapi.NativeFrame{View: u.attached.presentation.View()}}}
		if roster {
			terminal, _ := u.terminalViews.NewAttachment(u.shell.Transcript.Painter.Theme)
			terminal.Agents.UpdateRoster(u.attached.childSource.Publication(u.attached.presentation, u.attached.source))
			c.renderer.terminal = terminal
		}
		c.details(c.event.Frame.View.Version, sessionapi.NativeDetailRequest{Kind: sessionapi.NativeSkillDetails, Agent: true, Caller: "/root/gone"})
		if len(c.calls) != 0 || c.reattach != "" || c.attachOperation != nil || c.notice != sessionpresentation.ErrStaleIntent.Error() {
			t.Fatalf("missing skill caller became a local detail, roster=%v", roster)
		}
	}
}

func TestNativeTranscriptSheetCrossEventNavigationAndRestore(t *testing.T) {
	for _, initiallyFollowing := range []bool{true, false} {
		t.Run(fmt.Sprintf("following=%v", initiallyFollowing), func(t *testing.T) {
			workspace := t.TempDir()
			u := newAppServerSessionTestUI(t, workspace)
			makeEdit := func(id string, files ...string) {
				var changes []binding.FileChange
				for _, name := range files {
					change := binding.FileChange{
						Path: workspace + "/" + name,
						Diff: fmt.Sprintf("@@ -1,1 +1,1 @@\n-old %s\n+new %s\n", name, name),
					}
					change.Kind.Type = "update"
					changes = append(changes, change)
				}
				appServerTestNotify(t, u, "item/completed", map[string]any{
					"threadId": "main", "turnId": "work",
					"item": binding.ThreadItem{ID: id, Type: "fileChange", Status: "completed", Changes: changes},
				})
			}
			makeEdit("edit-ab", "a.go", "b.go")
			appServerTestNotify(t, u, "item/completed", map[string]any{
				"threadId": "main", "turnId": "work",
				"item": binding.ThreadItem{ID: "msg-1", Type: "agentMessage", Text: strings.Repeat("Middle message line.\n", 18)},
			})
			makeEdit("edit-cd", "c.go", "d.go")

			_, view, _ := nativeDetailTestViews(t, u)
			terminal, _ := u.terminalViews.NewAttachment(diffrender.DarkTheme)
			t.Cleanup(func() { _ = terminal.Diff.Close() })
			terminal.Width, terminal.Height = 120, 22
			client := nativeClient{renderer: nativeClientRenderer{terminal: terminal, views: u.terminalViews}}
			now := time.Unix(100, 0)

			publish := func() {
				frame := sessionapi.NativeFrame{
					View: view.attached.presentation.View(),
					Main: view.attached.source.Publication(view.attached.presentation, nil),
				}
				view.publishDetails(&frame)
				client.accept(sessionapi.NativeEvent{Attachment: "first", Frame: frame})
			}
			drain := func() {
				for len(client.calls) > 0 {
					call := client.calls[0]
					client.calls = client.calls[1:]
					if refusal := view.applyView(call.operation); refusal != "" {
						t.Fatalf("operation %+v refused: %s", call.operation, refusal)
					}
				}
				publish()
			}
			screen := vt.NewEmulator(terminal.Width, terminal.Height)
			defer screen.Close()
			render := func() string {
				var buf bytes.Buffer
				if _, err := client.renderer.render(t.Context(), &buf, now, "test"); err != nil {
					t.Fatal(err)
				}
				if _, err := screen.Write(buf.Bytes()); err != nil {
					t.Fatal(err)
				}
				return screen.String()
			}

			publish()
			render()

			// Scroll to top to click a.go, then set desired initial scroll/follow state.
			terminal.Transcript.Following = false
			terminal.Transcript.Offset = 0
			rendered := render()
			x, y := 0, 0
			for row, line := range strings.Split(rendered, "\n") {
				if before, _, ok := strings.Cut(line, "a.go"); ok {
					x, y = ansi.StringWidth(before)+1, row+1
					break
				}
			}
			if x == 0 {
				t.Fatalf("a.go row missing from transcript:\n%s", rendered)
			}

			savedOffset := 3
			if initiallyFollowing {
				terminal.Transcript.Follow()
			} else {
				terminal.Transcript.Following = false
				terminal.Transcript.Offset = savedOffset
			}
			client.dispatch(terminalsession.Input{Kind: terminalsession.MouseInput, Text: fmt.Sprintf("\x1b[<0;%d;%dM", x, y)})
			client.dispatch(terminalsession.Input{Kind: terminalsession.MouseInput, Text: fmt.Sprintf("\x1b[<0;%d;%dm", x, y)})
			drain()
			render()

			activePath := func() string {
				if terminal.Dialog == nil || len(terminal.Dialog.Pages) == 0 {
					return ""
				}
				page := terminal.Dialog.Pages[terminal.Dialog.Page]
				if page.DisplayPath != "" {
					return page.DisplayPath
				}
				return page.Path
			}
			checkStep := func(step, wantPath, wantTranscript string) {
				t.Helper()
				if got := activePath(); !strings.HasSuffix(got, wantPath) {
					t.Fatalf("after %s: active tab=%q, want suffix %q", step, got, wantPath)
				}
				rendered := render()
				// Check the transcript portion (above the bottom-docked dialog) contains wantPath.
				transcriptRows := strings.Split(rendered, "\n")
				if n := terminal.Transcript.FeedRows; n > 0 && n < len(transcriptRows) {
					transcriptRows = transcriptRows[:n]
				}
				if !strings.Contains(strings.Join(transcriptRows, "\n"), wantTranscript) {
					t.Fatalf("after %s: transcript did not scroll %q into view:\n%s", step, wantTranscript, rendered)
				}
			}
			sendKey := func(key string) {
				client.dispatch(terminalsession.Input{Kind: terminalsession.KeyInput, Text: key})
				drain()
				render()
			}

			checkStep("click a.go", "a.go", "a.go")
			// Boundary check at first tab of first event: Left stays on a.go.
			sendKey("\x1b[D")
			checkStep("left at start boundary", "a.go", "a.go")

			// a -> b -> c -> d
			sendKey("\x1b[C")
			checkStep("right to b.go", "b.go", "a.go")
			sendKey("\x1b[C")
			checkStep("right across message to c.go", "c.go", "c.go")
			sendKey("\x1b[C")
			checkStep("right to d.go", "d.go", "c.go")
			// Boundary check at last tab of last event: Right stays on d.go.
			sendKey("\x1b[C")
			checkStep("right at end boundary", "d.go", "c.go")

			// d <- c <- b <- a
			sendKey("\x1b[D")
			checkStep("left to c.go", "c.go", "c.go")
			sendKey("\x1b[D")
			checkStep("left across message to b.go (last tab)", "b.go", "a.go")
			sendKey("\x1b[D")
			checkStep("left to a.go", "a.go", "a.go")

			// Esc closes sheet and restores original Following / Offset and clears highlight.
			sendKey("\x1b")
			if terminal.Dialog != nil {
				t.Fatal("Esc did not close dialog")
			}
			if !terminal.Transcript.ActiveDetail.Zero() {
				t.Fatalf("ActiveDetail not cleared on close: %+v", terminal.Transcript.ActiveDetail)
			}
			if terminal.Transcript.Following != initiallyFollowing {
				t.Fatalf("Following=%v after close, want %v", terminal.Transcript.Following, initiallyFollowing)
			}
			if !initiallyFollowing && terminal.Transcript.Offset != savedOffset {
				t.Fatalf("Offset=%d after close, want %d", terminal.Transcript.Offset, savedOffset)
			}
		})
	}
}
