package executionobservation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"
)

func testCriticalErrors() *requestexecution.Failures {
	return requestexecution.NewFailures(new(notice.Service))
}
