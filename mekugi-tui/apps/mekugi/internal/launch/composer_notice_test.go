package launch

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/textsanitize"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"

	"github.com/charmbracelet/x/vt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
)

func TestComposerErrorClickAndDismiss(t *testing.T) {
	for _, tc := range []struct {
		kind  string
		width int
	}{
		// The attachment notice opens only when clipped, as it is at 40 columns.
		{"command", 12}, {"command", 13}, {"command", 60}, {"attachment", 40}, {"host", 60},
	} {
		t.Run(fmt.Sprintf("%s/%d", tc.kind, tc.width), func(t *testing.T) {
			u, wire := newAppServerTestUI()

			defer terminalTest(u).Diff.Close()
			u.Turn, u.attached.presentation.Status.Text, u.Text = "active", "Working", "keep draft"
			switch tc.kind {
			case "command":
				u.Text = "/unknown"
				appServerTestKeys(t, u, "\r")
			case "attachment":
				setNotice(&u.attached.feedback, presentationTime(u.clock), "Selection exceeds the mention limit", true)
			case "host":
				appServerTestMessage(t, u, `{"method":"turn/completed","params":{"threadId":"main","turn":{"id":"active","status":"failed","error":{"message":"provider failed\nFull diagnostics"}}}}`)
			}
			full, draft, turn, sent := u.composerErrorText(), u.Text, u.Turn, wire.String()
			screen := vt.NewEmulator(tc.width, 16)
			defer screen.Close()
			if err := u.testPaint(screen, tc.width, 16); err != nil {
				t.Fatal(err)
			}
			click := func(rect terminalsession.Rect) {
				t.Helper()
				if rect.W == 0 {
					t.Fatal("missing error control")
				}
				x, y := rect.X+terminalTest(u).Layout.Main.X+1, rect.Y+terminalTest(u).Layout.Main.Y+1
				if err := terminalTest(u).mouse(fmt.Sprintf("\x1b[<0;%d;%dM", x, y)); err != nil {
					t.Fatal(err)
				}
			}
			// The details control ends the clickable label.
			details := u.shell.Main.NoticeDetails
			details.X, details.W = details.X+details.W-1, min(1, details.W)
			click(details)
			if terminalTest(u).Dialog == nil {
				t.Fatal("truncated error did not open")
			}
			drawOutputDialog(terminalTest(u))
			if terminalTest(u).Dialog.Document.Text != textsanitize.Safe(full, false) {
				t.Fatal("details lost full error")
			}
			terminalTest(u).outputKey("q")
			click(u.shell.Main.NoticeDismiss)
			if u.composerErrorText() != "" || u.Text != draft || u.Turn != turn || wire.String() != sent || len(u.attached.source.Entries) != 0 {
				t.Fatal("dismissal changed draft, host state, wire, or transcript")
			}
		})
	}
}

func TestUISnapshotComposerError(t *testing.T) {
	for _, width := range []int{12, 20, 36, 80} {
		t.Run(fmt.Sprint(width), func(t *testing.T) {
			u, _ := newAppServerTestUI()
			u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
			u.clock = func() time.Time { return time.Date(2026, 10, 6, 12, 0, 0, 0, time.UTC) }
			u.attached.presentation.Status.Text = "Ready"
			setNotice(&u.attached.feedback, presentationTime(u.clock), "File contents omitted: setup.sh (attachment budget exceeded).\nThe attachment result names the missing content.", true)
			rows, _ := u.testMainFrame(width, 8, 0)
			assertNativeUISnapshot(t, fmt.Sprintf("composer-error-%d", width), rows)
			// Dismissal is a neutral control, not a success.
			if !strings.Contains(rows[u.shell.Main.NoticeDismiss.Y], terminalui.Dim+"[×]") {
				t.Fatal("dismiss control is not dim")
			}
		})
	}
	u, _ := newAppServerTestUI()
	setNotice(&u.attached.feedback, presentationTime(u.clock), "failed", true)
	u.testMainFrame(80, 8, 0)
	setNotice(&u.attached.feedback, presentationTime(u.clock), "done", false)
	u.testMainFrame(80, 8, 0)
	if u.shell.Main.NoticeDetails.W != 0 || u.shell.Main.NoticeDismiss.W != 0 {
		t.Fatal("stale error controls survived feedback")
	}
}
