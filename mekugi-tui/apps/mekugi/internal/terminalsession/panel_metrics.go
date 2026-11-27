package terminalsession

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
)

// Source: apps/mekugi/internal/router/app_server_session_metrics.go:97:299@630ce384.
func (p *StatusPanel) renderMetrics() {
	p.Controls = "tab/⇧tab views · ↑↓ scroll · r refresh"
	p.Title = "Session · Overview"
	p.Fields = p.body.Fields
	if p.Features {
		p.Title = "Session · Features"
		p.Fields = p.body.FeatureFields
		if len(p.Fields) == 0 {
			p.Fields = []sessionpresentation.StatusField{{Group: "Features", Value: "Loading feature counts…"}}
		}
	}
	p.Groups = nil
	for _, field := range p.Fields {
		if !slices.Contains(p.Groups, field.Group) {
			p.Groups = append(p.Groups, field.Group)
		}
	}
}
