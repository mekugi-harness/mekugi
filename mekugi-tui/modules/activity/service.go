package activity

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
)

type Deps struct{}

func newService(_ Deps, _ module.Slots) (*feed.Service, error) {
	return new(feed.Service), nil
}
