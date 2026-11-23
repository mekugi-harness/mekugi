package sessionnaming

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionnaming/internal/naming"
)

type Deps struct{}

func newService(Deps, module.Slots) (naming.Service, error) { return naming.Service{}, nil }
