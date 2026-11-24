package launch

import (
	"strings"
	"testing"
	"time"

	previewview "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/terminalview"

	"github.com/charmbracelet/x/vt"
	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

func TestNativeUIExecEditCompletionRetainsWorkingStatusAndClearsDock(t *testing.T) {
	t.Parallel()
	u, _ := newAppServerTestUI()

	defer terminalTest(u).Diff.Close()
	u.attached.presentation.Status.Text = "Working"
	workspace := t.TempDir()
	preview := previewmodel.Preview{ID: "running:edit-then-test", Workspace: workspace, Thread: "thread", Caller: "/root", Tool: nativeExecCommandToolName, Status: previewmodel.PreviewRunning, Input: "observed edit"}
	applyTestPreview(&u.attached, u.shell, u.review, &u.pendingEditReceipts, presentationTime(u.clock), preview, false)
	revealNativeDock(terminalTest(u))
	preview.Complete = true
	applyTestPreview(&u.attached, u.shell, u.review, &u.pendingEditReceipts, presentationTime(u.clock), preview, false)
	revealNativeDock(terminalTest(u))
	if terminalTest(u).Live.Live() != 0 || u.attached.presentation.Status.Text != "Working" {
		t.Fatal("edit completion changed process status or retained an active dock")
	}
	screen := vt.NewEmulator(120, 40)
	defer screen.Close()
	if err := u.testPaint(screen, 120, 40); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(screen.String(), "LIVE ·") {
		t.Fatal("completed edit was not rendered during its minimum display time")
	}
	terminalTest(u).Live.Tick(time.Now().Add(previewview.CompletionHold + time.Millisecond))
	if len(terminalTest(u).Live.Order) != 0 || u.attached.presentation.Status.Text != "Working" {
		t.Fatal("completed dock did not clear independently of process Working status")
	}
	// A final host observation must not create a second, late completion card.
	applyTestPreview(&u.attached, u.shell, u.review, &u.pendingEditReceipts, presentationTime(u.clock), previewmodel.Preview{ID: "late:edit-then-test", Workspace: workspace, Tool: nativeExecCommandToolName, Complete: true, Status: previewmodel.PreviewRunning}, false)
	revealNativeDock(terminalTest(u))
	if len(terminalTest(u).Live.Order) != 0 {
		t.Fatal("late shell completion resurrected the dock")
	}
}
