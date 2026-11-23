package usage

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
	usagepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/presentation"
)

var Module = module.Descriptor[Deps, *accounting.Service, module.NoBindings, module.NoViews]{
	ID: "usage", Summary: "Retain provider-authoritative thread usage, reference costs and throughput",
	Requires: []module.ID{"sessionhistory"}, Lifetime: module.Invocation,
	Resources:   []string{"on-demand retained-usage publication worker"},
	Contributes: module.Contributions[*accounting.Service]{RecordKinds: accounting.RecordKinds(), Presentation: []presentation.Status{usagepresentation.Throughput}},
	New:         newService, Start: startService, Stop: stopService,
}
