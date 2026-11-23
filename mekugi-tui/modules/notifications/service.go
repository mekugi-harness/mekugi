package notifications

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"
)

type Deps struct{}

func newService(Deps, module.Slots) (*notice.Service, error) { return new(notice.Service), nil }
