package conversation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

type Deps struct{}

func newService(Deps, module.Slots) (*presentation.Service, error) {
	return new(presentation.Service), nil
}
