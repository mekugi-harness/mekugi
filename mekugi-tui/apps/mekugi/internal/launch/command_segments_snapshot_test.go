package launch

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

func TestUISnapshotCommandSegmentRetentionNotice(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
	}{
		{"command-segments-storage-busy", context.DeadlineExceeded},
		{"command-segments-storage-failure", requestexecution.StorageIOError(errors.New("open /workspace/replay/call-record.json: permission denied\n" + strings.Repeat("diagnostic detail ", 12) + "final-cause-marker"))},
	} {
		t.Run(tc.name, func(t *testing.T) {
			u, _ := newAppServerTestUI()
			u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
			u.clock = func() time.Time { return time.Date(2026, 9, 30, 12, 0, 0, 0, time.Local) }
			u.Thread = "main"
			u.shell.Transcript.Conversation = true
			u.testActivity(u.shell.Transcript).testAudience()
			u.commandSegmentPending = 1
			commandSegmentRetained(&u.commandSegmentPending, u.issues, commandSegmentWrite{key: [3]string{"main", "turn", "command"}, err: tc.err})
			delivery := applyCriticalNotices(&u.attached, &u.Client, &u.picker, &u.Editor, u.clock, u.settings, u.title, u.backendVersion, &u.restoring, &u.side, u.audience, u.issues)
			if delivery == nil || !strings.Contains(u.attached.feedback.Text, tc.err.Error()) {
				t.Fatal("composer omitted the underlying error")
			}
			rows, receipt := u.testDeliveredMainFrame(t, 80, 12, 0)
			assertNativeUISnapshot(t, "command-segments-storage-failure", rows)
			delivery.finish(receipt)
		})
	}
}
