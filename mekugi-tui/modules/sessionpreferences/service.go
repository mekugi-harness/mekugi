package sessionpreferences

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences/preferences"
)

type Deps struct{ History *history.Service }

func newService(deps Deps, _ module.Slots) (*preferences.Service, error) {
	return preferences.New(deps.History)
}
