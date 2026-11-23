package executionobservation

import (
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"

	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

func waitLiveDiffWorkerPreview(t *testing.T, broker *previewmodel.Session, sub *sessionreview.Subscription, match func(previewmodel.Preview) bool) previewmodel.Preview {
	t.Helper()
	timer := time.NewTimer(5 * time.Second)
	defer timer.Stop()
	for {
		select {
		case <-sub.Ready():
			for _, event := range testReview(broker).TakePreviews(sub) {
				if event.Preview != nil && match(*event.Preview) {
					return *event.Preview
				}
			}
		case <-timer.C:
			t.Fatal("missing live diff worker preview")
		}
	}
}
