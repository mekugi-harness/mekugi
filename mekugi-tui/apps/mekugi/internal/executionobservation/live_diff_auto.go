package executionobservation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"

	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

func observeReview(service *sessionreview.Review, workspace, thread string, metadata codexresponses.TurnMetadata) {
	if metadata.ActivityIdentityInvalid || metadata.RequestKind != "turn" {
		return
	}
	service.Observe(workspace, thread, metadata.ParentThreadID, metadata.SubagentKind == "")
}

func beginLiveDiffTurn(service *previewmodel.Session, workspace, thread string, metadata codexresponses.TurnMetadata) {
	if metadata.ActivityIdentityInvalid || metadata.RequestKind != "turn" || metadata.SubagentKind != "" || metadata.ThreadID != "" && metadata.ThreadID != thread {
		return
	}
	service.BeginTurn(workspace, thread, metadata.TurnID)
}
