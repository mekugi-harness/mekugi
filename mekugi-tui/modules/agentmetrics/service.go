package agentmetrics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentmetrics/evidence"
)

type Deps struct{}

func newService(Deps, module.Slots) (*evidence.Service, error) { return new(evidence.Service), nil }
