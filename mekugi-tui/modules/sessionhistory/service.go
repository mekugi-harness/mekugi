package sessionhistory

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func newService(deps Deps, slots module.Slots) (*history.Service, error) {
	return history.NewService(append(slots.RecordKinds, deps.RecordKinds...))
}
