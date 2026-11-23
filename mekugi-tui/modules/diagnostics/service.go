package diagnostics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessioninspection/query"
)

type Deps struct {
	History    *history.Service
	Inspection *query.Service
}

func newService(deps Deps, slots module.Slots) (*diagnostic.Service, error) {
	return diagnostic.New(deps.History, deps.Inspection, slots.DiagnosticSections)
}
