package agentmetrics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/diagnosticsection"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inspection"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"
)

var Module = module.Descriptor[Deps, *evidence.Service, module.NoBindings, module.NoViews]{
	ID: "agentmetrics", Summary: "Measure reader invocations, observed edits and recorded host completion",
	Lifetime: module.Stateless, New: newService,
	Contributes: module.Contributions[*evidence.Service]{DiagnosticSections: []diagnosticsection.Definition[*evidence.Service]{{PrepareReads: (*evidence.Service).PrepareDiagnosticReads, ReadSnapshot: (*evidence.Service).DiagnosticReads}}, Inspection: []inspection.Definition[*evidence.Service]{{NewAX: func(service *evidence.Service) inspection.AXCollector { return service.NewInspection(nil) }}}},
}
