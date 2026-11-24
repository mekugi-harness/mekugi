package launch

import (
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"
)

func queueCritical(c *requestexecution.Failures, session string) {
	c.AddThreadNotice(session, session, "unsupported_tool_catalog", "Enable supported tools.")
}

func TestCriticalErrorsDeduplicateReserveAndRetainUntilDelivery(t *testing.T) {
	c := testCriticalErrors()
	queueCritical(c, "one")
	queueCritical(c, "one")
	if pending := c.Notices.Pending(); len(pending) != 1 || !strings.Contains(pending[0], "2 times") {
		t.Fatalf("pending = %v", pending)
	}
	first := claimNativeNotices(c, "one", nil)
	if first == nil || len(first.Errors.Messages) != 1 || claimNativeNotices(c, "two", nil) != nil || claimNativeNotices(c, "one", nil) != nil {
		t.Fatal("notice crossed a session or concurrent response")
	}
	first.Errors.Finish(false)
	retry := claimNativeNotices(c, "one", nil)
	if len(retry.Errors.Messages) != 1 || !strings.Contains(retry.Errors.Messages[0].Text, "Enable supported tools.") {
		t.Fatal("failed delivery lost notice")
	}
	retry.Errors.Finish(true)
	if len(c.Notices.Pending()) != 0 {
		t.Fatal("delivered notice still pending")
	}
	queueCritical(c, "one")
	if claimNativeNotices(c, "one", nil) != nil {
		t.Fatal("repeat flooded the session")
	}
	if pending := c.Notices.Pending(); len(pending) != 1 || !strings.Contains(pending[0], "3 times") {
		t.Fatalf("repeat summary = %v", pending)
	}
}

func TestCriticalErrorsNativeOwnershipAndRouterWideNotice(t *testing.T) {
	c := testCriticalErrors()
	queueCritical(c, "one")
	queueCritical(c, "two")
	queueCritical(c, "child")
	c.AddNotice("", "router-capacity", "router-wide capacity warning")
	activity := sessioncontrol.NewAudience(nil)
	if !activity.Observe("one", "", "/root", false) || !activity.Observe("child", "one", "/root/child", true) {
		t.Fatal("failed to establish ancestry")
	}
	child := claimNativeNotices(c, "child", activity)
	if child != nil {
		t.Fatal("child stole native root notices")
	}
	root := claimNativeNotices(c, "one", activity)
	if root == nil || len(root.Errors.Messages) != 3 {
		t.Fatalf("root notices = %d, want own, child, and router-wide", len(root.Errors.Messages))
	}
	root.Errors.Finish(true)
	if len(c.Notices.Pending()) != 1 {
		t.Fatalf("unrelated session notice was acknowledged: %v", c.Notices.Pending())
	}
}

func testCriticalErrors() *requestexecution.Failures {
	return requestexecution.NewFailures(new(notice.Service))
}
