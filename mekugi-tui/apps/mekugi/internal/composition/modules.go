package composition

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	activityview "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
	changeview "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/terminalview"
)

// NativeRenderer borrows only statically selected painters. It constructs no
// history, capture, host, provider or guard owner.
func NativeRenderer(childReplySummary bool) (terminalsession.Views, error) {
	auxiliary := []terminalui.PaneFactory[*scope.Store]{{
		Name:  "saved-review",
		Label: "Diff",
		Attach: func(theme diffrender.Theme) terminalui.PaneAttachment[*scope.Store] {
			return terminalui.PaneAttachment[*scope.Store]{Pane: changeview.NewPane("", theme)}
		},
	}}
	roster := []terminalui.RosterDefinition[*activityview.View]{{
		Name:  "activity",
		Label: "Agents",
		New:   activityview.NewView,
	}}
	views, err := terminalsession.NewViews(auxiliary, roster)
	views.ChildReplySummary = childReplySummary
	return views, err
}
