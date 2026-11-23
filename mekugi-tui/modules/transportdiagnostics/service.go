package transportdiagnostics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

type Deps struct{}

func newService(Deps, module.Slots) (*capture.Service, error) { return new(capture.Service), nil }
