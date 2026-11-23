package usage

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
)

type Deps struct{}

func newService(_ Deps, _ module.Slots) (*accounting.Service, error) {
	return accounting.New(), nil
}
func startService(ctx context.Context, s *accounting.Service) error { return s.Start(ctx) }
func stopService(_ context.Context, s *accounting.Service) error    { s.Close(); return nil }
