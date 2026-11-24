package launch

import (
	"encoding/base64"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"fmt"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/vt"

	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

func assertErrorDialog(t *testing.T, u *terminalUI, full string) {
	t.Helper()
	drawOutputDialog(u)
	if u.Dialog.Document.Text != full {
		t.Fatal("dialog copy target is not the complete sanitized error")
	}
	u.outputKey("/")
	u.outputKey("forced cleanup")
	u.outputKey("\r")
	if frame := drawOutputDialog(u); strings.Contains(frame, "no match") || !strings.Contains(frame, "forced cleanup") {
		t.Fatal("could not search the decisive suffix beyond the inline preview")
	}
	u.outputKey("y")
	want := "\x1b]52;c;" + base64.StdEncoding.EncodeToString([]byte(full)) + "\x07"
	if u.Frame.Clipboard != want {
		t.Fatal("whole-error copy dropped retained evidence")
	}
}

func TestErrorDetailsRestoredRollout(t *testing.T) {
	for _, child := range []bool{false, true} {
		t.Run(map[bool]string{false: "main", true: "child"}[child], func(t *testing.T) {
			u := newAppServerSessionTestUI(t, t.TempDir())
			u.Startup.Thread = "main"
			u.shell.Agents = terminalTest(u).Agents
			detail := "CreateProcess: " + strings.Repeat("command ", 9000) + "\nrejected: forced cleanup is not permitted\n    at cell:17"
			records := append([]map[string]any{rolloutRecord(1, "event_msg", map[string]any{"type": "task_started", "turn_id": "t"})}, rolloutFailedCell(2, "rejected", detail)...)
			thread := "main"
			if child {
				thread = "child"
				u.Thread = thread
				u.startSession(thread, u.attached.cwd)
			}
			info := binding.ThreadInfo{ID: thread, Cwd: u.attached.cwd, Path: writeTestRollout(t, thread, records...), Turns: []binding.HistoryTurn{{ID: "t", Status: "failed"}}}
			if err := u.restoreContent(info); err != nil {
				t.Fatal(err)
			}
			view := u.testActivity(u.shell.Transcript)
			full := "exec script failed: " + detail
			index := slices.IndexFunc(view.Entries, func(e sessionpresentation.ActivityRecord) bool {
				return e.Source.Kind == "error" && e.Source.CallID == "rejected"
			})
			if index < 0 || view.Entries[index].Source.ErrorDetail != full || view.Entries[index].Source.Text != activitypresentation.ErrorPreview(full) {
				t.Fatal("restoration clipped the full host error")
			}
			if !terminalTest(u).openEntry(view.bind(terminalTest(u)), view.Entries[index].Source.Seq) {
				t.Fatal("restored error has no dialog")
			}
			assertErrorDialog(t, terminalTest(u), full)
		})
	}
}

func TestErrorDetailsKeyboardPaginationAndFiltering(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	u.testActivity(u.shell.Agents).setDisplayFilter(true, "/root/worker")
	u.testActivity(u.shell.Agents).Apply(sessionpresentation.ActivityEvent{Entries: []sessionpresentation.ActivityEntry{
		{Seq: 1, Agent: "/root/a", Kind: "error", Text: "old error"},
		{Seq: 2, Agent: "/root/b", Kind: "error", Text: "other agent"},
		{Seq: 3, Agent: "/root/a", Kind: "error", Text: "new error"},
	}})
	u.testActivity(u.shell.Agents).setDisplayFilter(true, "/root/a")
	terminalTest(u).openErrors()
	if terminalTest(u).Dialog == nil || len(terminalTest(u).Dialog.Pages) != 2 || terminalTest(u).Dialog.Page != 1 {
		t.Fatal("keyboard error details lost selection or filter")
	}
	drawOutputDialog(terminalTest(u))
	if terminalTest(u).Dialog.Document.Text != "new error" {
		t.Fatal("keyboard did not open newest error")
	}
	terminalTest(u).outputKey("\x1b[Z")
	drawOutputDialog(terminalTest(u))
	if terminalTest(u).Dialog.Document.Text != "old error" {
		t.Fatal("could not navigate to earlier error")
	}
}

func TestErrorDetailsSearchAcrossVisualWraps(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	query := strings.Repeat("q", 80) + " forced cleanup"
	// The tail keeps the next match beyond the first match's viewport.
	full := strings.Repeat("prefix ", 160) + query + strings.Repeat(" tail", 240) + "\n" + query
	u.testActivity(u.shell.Transcript).Apply(sessionpresentation.ActivityEvent{Entries: []sessionpresentation.ActivityEntry{{Seq: 1, Agent: "Main", Kind: "error", Text: full}}})
	if !terminalTest(u).openEntry(u.shell.Transcript, 1) {
		t.Fatal("could not open error")
	}
	drawOutputDialog(terminalTest(u))
	terminalTest(u).outputKey("/")
	terminalTest(u).outputKey(query)
	terminalTest(u).outputKey("\r")
	first := drawOutputDialog(terminalTest(u))
	if !strings.Contains(first, "qqqq") {
		t.Fatal("matched text was not scrolled into view")
	}
	terminalTest(u).outputKey("n")
	if next := drawOutputDialog(terminalTest(u)); next == first || !strings.Contains(next, "qqqq") {
		t.Fatal("next did not reach the next logical match")
	}
	terminalTest(u).outputKey("N")
	if drawOutputDialog(terminalTest(u)) != first {
		t.Fatal("previous did not return to the wrapped match")
	}
	assertErrorDialog(t, terminalTest(u), full)
}

func TestErrorDetailsTerminalClick(t *testing.T) {
	for _, child := range []bool{false, true} {
		t.Run(map[bool]string{false: "main", true: "child"}[child], func(t *testing.T) {
			u := newAppServerSessionTestUI(t, t.TempDir())
			view, agent := u.testActivity(u.shell.Transcript), "Main"
			if child {
				view, agent = u.testActivity(u.shell.Agents), "/root/worker"
				u.testActivity(u.shell.Agents).setDisplayFilter(true, "/root/worker")
			}
			full := "Failed: " + strings.Repeat("diagnostic ", 40) + "\nrejected: forced cleanup is not permitted"
			view.Apply(sessionpresentation.ActivityEvent{Entries: []sessionpresentation.ActivityEntry{{Seq: 1, Agent: agent, Kind: "error", Text: full}}})
			u.Text = "preserve draft"
			if child {
				u.testActivity(u.shell.Agents).setDisplayFilter(true, agent)
			}
			screen := vt.NewEmulator(120, 40)
			defer screen.Close()
			if err := u.testPaint(screen, 120, 40); err != nil {
				t.Fatal(err)
			}
			row := slices.IndexFunc(view.FeedSnippets, func(s activityui.Snippet) bool { return s != activityui.Snippet{} })
			if row < 0 {
				t.Fatal("painted error lacks pointer target")
			}
			pane := terminalTest(u).Layout.Main
			if child {
				pane = terminalTest(u).Layout.Main
			}
			x, y := pane.X+view.FeedLeft, pane.Y+view.FeedTop+row
			offset, following := view.Offset, view.Following
			for _, suffix := range []string{"M", "m"} {
				if err := terminalTest(u).mouse(fmt.Sprintf("\x1b[<0;%d;%d%s", x, y, suffix)); err != nil {
					t.Fatal(err)
				}
			}
			if terminalTest(u).Dialog == nil || u.Text != "preserve draft" || view.Offset != offset || view.Following != following {
				t.Fatal("terminal click did not open details without disturbing the pane")
			}
			assertErrorDialog(t, terminalTest(u), full)
		})
	}
}

func TestUISnapshotErrorDetailsConversation(t *testing.T) {
	view := newActivityAdapter()
	view.Conversation = true
	view.testAudience()
	view.Clock = func() time.Time { return time.Date(2026, 10, 2, 12, 40, 21, 0, time.Local) }
	view.Apply(sessionpresentation.ActivityEvent{Entries: []sessionpresentation.ActivityEntry{{Seq: 1, Agent: "Main", Kind: "error", Observed: view.Now(),
		Text: "exec script failed", ErrorDetail: "exec script failed: exec_command failed: CreateProcess { message: Rejected(\"" + strings.Repeat("escaped command ", 30) + "\") }\nrejected: forced cleanup is not permitted"}}})
	assertNativeUISnapshot(t, "error-details-conversation", view.RenderFeed(80, 20).Lines)
}
