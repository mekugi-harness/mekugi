package commandsegments

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/reports"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/segments"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

type Deps struct {
	History *history.Service
	Live    bool
}

var Module = module.Descriptor[Deps, *segments.Service, module.NoBindings, module.NoViews]{
	ID: "commandsegments", Summary: "Receive, retain, and restore command-segment observations",
	Requires: []module.ID{"sessionhistory"}, Lifetime: module.Invocation,
	Resources:   []string{"optional private report FIFOs and consumer tasks"},
	Contributes: module.Contributions[*segments.Service]{RecordKinds: reports.RecordKinds()},
	New:         newService, Start: startService, Stop: stopService,
}
