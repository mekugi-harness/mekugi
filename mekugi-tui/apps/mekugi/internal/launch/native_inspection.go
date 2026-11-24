package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
)

func (session *nativeSession) observeInspection(inspection *sessioncontrol.Inspection) {
	if session.Client.Requests.DispatchMeasurement(inspection) {
		return
	}
	target := inspection.Target()
	var native *binding.ThreadInfo
	if connection := session.connection; connection != nil && connection.root == session {
		native = connection.agent(target)
	}
	ownedNative := native != nil && native.IsLoaded()
	switch {
	case target == session.Client.Thread:
		inspection.Complete(target, threadInspection(&session.attached.session, target, session.settings))
	case session.Client.Requests.HasSideThread(target):
		inspection.Complete(target, observation.SessionDetails{SessionMetrics: session.attached.session.InspectionMetrics(target)})
	case ownedNative:
		inspection.Complete(target, native.Details())
	default:
		inspection.Complete(session.Client.Thread, observation.SessionDetails{})
	}
}
