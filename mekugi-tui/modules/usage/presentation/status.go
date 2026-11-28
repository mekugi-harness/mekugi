package presentation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/presentation"
)

var Throughput = presentation.Status{
	ID:         "usage.throughput",
	Placements: []presentation.Placement{presentation.ComposerCaption, presentation.AgentMetric},
}
