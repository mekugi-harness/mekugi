package sessioninspection

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessioninspection/query"
)

type Deps struct{ History *history.Service }

func newService(deps Deps, slots module.Slots) (*query.Service, error) {
	return query.New(deps.History, slots.Inspection)
}
