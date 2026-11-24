package terminalsession

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
)

// Source: apps/mekugi/internal/router/app_server_session_metrics.go:97:299@630ce384.
func (p *StatusPanel) renderMetrics() {
	p.Title = "Session · " + sessionpresentation.MetricViews[p.Tab]
	p.Controls = "tab/⇧tab views · ↑↓ scroll · r refresh"
	if p.Tab == 2 {
		p.Controls = "tab/⇧tab views · [ older / ] newer · ↑↓ scroll"
	}
	p.Fields, p.Groups = nil, nil
	p.Fields = sessionpresentation.MetricFields(p.body, p.Tab, p.exchangeIndex())
	for i := range p.Fields {
		field := &p.Fields[i]
		if field.Group == "Exchange" && field.Label == "Retained" {
			field.Value += " · [ older / ] newer"
		}
		if !slices.Contains(p.Groups, field.Group) {
			p.Groups = append(p.Groups, field.Group)
		}
	}
}
