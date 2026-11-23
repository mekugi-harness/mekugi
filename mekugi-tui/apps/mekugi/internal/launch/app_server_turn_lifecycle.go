package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/executionobservation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
)

// Native turn completion retires display state even when interrupted tools
// never return. It neither finalizes capture nor stops a host-owned process.
func endTurnPreviews(windows *executionobservation.Windows, review *sessionreview.Review, workspace string, thread, turn string) {
	windows.StopTurnPreviews(thread, turn)
	if live := review.Preview(); live != nil {
		live.SetTurn(thread, turn, false)
		live.FinishTurn(workspace, thread, turn)
	}
}
