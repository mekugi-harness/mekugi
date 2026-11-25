package launch

import (
	"time"

	noticepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/presentation"
)

// Successful feedback is transient; actionable errors remain until editing.
func setNotice(feedback *noticepresentation.Model, at time.Time, text string, alert bool) {
	feedback.Set(text, alert, at)
}
