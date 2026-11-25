package composition

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage"
)

// NativeRenderer borrows only statically selected painters. It constructs no
// history, capture, host, provider or guard owner.
func NativeRenderer(childReplySummary bool) (terminalsession.Views, error) {
	c := frontendDeclarations()
	views, err := c.RendererViews()
	views.ChildReplySummary = childReplySummary
	return views, err
}

func frontendDeclarations() *Composition {
	var c Composition
	c.Add(conversation.Module, func() conversation.Deps { return conversation.Deps{} })
	c.Add(activity.Module, func() activity.Deps { return activity.Deps{} })
	c.Add(notifications.Module, func() notifications.Deps { return notifications.Deps{} })
	c.Add(agentmetrics.Module, func() agentmetrics.Deps { return agentmetrics.Deps{} })
	c.Add(sessionhistory.Module, func() sessionhistory.Deps { return sessionhistory.Deps{} })
	c.Add(transportdiagnostics.Module, func() transportdiagnostics.Deps { return transportdiagnostics.Deps{} })
	c.Add(changehistory.Module, func() changehistory.Deps { return changehistory.Deps{} })
	c.Add(sessionpreferences.Module, func() sessionpreferences.Deps { return sessionpreferences.Deps{} })
	c.Add(usage.Module, func() usage.Deps { return usage.Deps{} })
	c.Add(livepreview.Module, func() livepreview.Deps { return livepreview.Deps{} })
	c.Add(commandsegments.Module, func() commandsegments.Deps { return commandsegments.Deps{} })
	c.Add(diagnostics.Module, func() diagnostics.Deps { return diagnostics.Deps{} })
	return &c
}
