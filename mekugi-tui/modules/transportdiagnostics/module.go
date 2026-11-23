package transportdiagnostics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/diagnosticsection"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inspection"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

var Module = module.Descriptor[Deps, *capture.Service, module.NoBindings, module.NoViews]{
	ID: "transportdiagnostics", Summary: "Observe transport bytes, timing, capture health and provider-response evidence",
	Requires: []module.ID{"sessionhistory"}, Lifetime: module.Invocation, New: newService,
	Contributes: module.Contributions[*capture.Service]{RecordKinds: []recordkind.Spec{capture.TimingRecord}, DiagnosticSections: []diagnosticsection.Definition[*capture.Service]{{Transport: &diagnosticsection.Transport{Record: capture.DiagnosticRecord, Metrics: capture.DiagnosticMetrics}}}, Inspection: []inspection.Definition[*capture.Service]{{ProviderUsage: (*capture.Service).InspectUsage, RequestTiming: (*capture.Service).InspectTiming}}},
}
