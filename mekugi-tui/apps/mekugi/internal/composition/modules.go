package composition

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessioninspection"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionnaming"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage"
)

// Frontends composes feature-owned commands and records for the authenticated
// frontend host, including retained host facts and Activity snapshots.
func Frontends(ctx context.Context, liveReports, childReplySummary bool, captureDirectory string) (*Runtime, error) {
	c, bind := frontendDeclarations(liveReports, childReplySummary, captureDirectory)
	runtime, err := c.Activate(ctx, new(binding.Registry))
	if err != nil {
		return nil, err
	}
	bind(runtime)
	return runtime, nil
}

// NativeRenderer borrows only statically selected painters. It constructs no
// history, capture, host, provider or guard owner.
func NativeRenderer(childReplySummary bool) (terminalsession.Views, error) {
	c, _ := frontendDeclarations(false, childReplySummary, "")
	views, err := c.RendererViews()
	views.ChildReplySummary = childReplySummary
	return views, err
}

func frontendDeclarations(liveReports, childReplySummary bool, captureDirectory string) (*Composition, func(*Runtime)) {
	var c Composition
	conversation := c.Add(conversation.Module, func() conversation.Deps { return conversation.Deps{} })
	activity := c.Add(activity.Module, func() activity.Deps { return activity.Deps{} })
	notifications := c.Add(notifications.Module, func() notifications.Deps { return notifications.Deps{} })
	metrics := c.Add(agentmetrics.Module, func() agentmetrics.Deps { return agentmetrics.Deps{} })
	history := c.Add(sessionhistory.Module, func() sessionhistory.Deps {
		return sessionhistory.Deps{}
	})
	transport := c.Add(transportdiagnostics.Module, func() transportdiagnostics.Deps { return transportdiagnostics.Deps{} })
	changes := c.Add(changehistory.Module, func() changehistory.Deps {
		return changehistory.Deps{History: history.Value(), Directory: captureDirectory}
	})
	replay := c.Add(sessionreplay.Module, func() sessionreplay.Deps {
		return sessionreplay.Deps{History: history.Value(), Timing: transport.Value().ReplayTiming}
	})
	prefs := c.Add(sessionpreferences.Module, func() sessionpreferences.Deps { return sessionpreferences.Deps{History: history.Value()} })
	usage := c.Add(usage.Module, func() usage.Deps { return usage.Deps{} })

	live := c.Add(livepreview.Module, func() livepreview.Deps { return livepreview.Deps{ReadEndpoint: capture.ReadFile} })
	segments := c.Add(commandsegments.Module, func() commandsegments.Deps { return commandsegments.Deps{History: history.Value(), Live: liveReports} })
	c.Add(sessionnaming.Module, func() sessionnaming.Deps { return sessionnaming.Deps{} })
	inspection := c.Add(sessioninspection.Module, func() sessioninspection.Deps { return sessioninspection.Deps{History: history.Value()} })
	diagnosis := c.Add(diagnostics.Module, func() diagnostics.Deps {
		return diagnostics.Deps{History: history.Value(), Inspection: inspection.Value()}
	})
	return &c, func(runtime *Runtime) {
		runtime.Terminal.ChildReplySummary = childReplySummary
		runtime.Replay = replay.Value()
		runtime.Inspection = inspection.Value()
		runtime.TransportDiagnostics = transport.Value()
		runtime.Diagnostics = diagnosis.Value()
		runtime.AgentMetrics = metrics.Value()
		runtime.Usage = usage.Value()
		runtime.Changes = changes.Value()
		runtime.History = history.Value()
		runtime.Preferences = prefs.Value()
		runtime.Notifications = notifications.Value()
		runtime.Activity = activity.Value()
		runtime.Conversation = conversation.Value()
		runtime.LivePreview = live.Value()
		runtime.Segments = segments.Value()
	}
}
