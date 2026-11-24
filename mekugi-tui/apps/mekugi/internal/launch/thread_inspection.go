package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences/preferences"
)

func threadInspection(session *sessionpresentation.Attachment, thread string, applied preferences.Applied) observation.SessionDetails {
	details := observation.SessionDetails{SessionMetrics: session.InspectionMetrics(thread)}
	if applied.Model != "" {
		details.Model = new(applied.Model)
		details.ReasoningEffort = new(applied.Effort)
		details.ServiceTier = new(applied.Tier)
	}
	return details
}

func restoreCompactionMetrics(session *sessionpresentation.Attachment, info binding.ThreadInfo) {
	var completed []string
	for _, turn := range info.Turns {
		for _, item := range turn.Items {
			if item.Type == "contextCompaction" && sessionpresentation.CompletedCompaction(item.Status) {
				completed = append(completed, item.ID)
			}
		}
	}
	session.RestoreCompactions(info.ID, completed)
}
