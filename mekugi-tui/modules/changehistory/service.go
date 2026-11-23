package changehistory

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
)

func newService(deps Deps, _ module.Slots) (*capture.Service, error) {
	return capture.New(deps.History, deps.Directory)
}

func startService(ctx context.Context, service *capture.Service) error { return service.Start(ctx) }
func stopService(ctx context.Context, service *capture.Service) error  { return service.Stop(ctx) }
