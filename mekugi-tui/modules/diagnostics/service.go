package diagnostics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

type Deps struct {
	History *history.Service
}

func newService(deps Deps, slots module.Slots) (*diagnostic.Service, error) {
	return diagnostic.New(deps.History, slots.DiagnosticSections)
}
