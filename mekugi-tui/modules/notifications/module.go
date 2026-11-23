package notifications

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"
)

var Module = module.Descriptor[Deps, *notice.Service, module.NoBindings, module.NoViews]{
	ID: "notifications", Summary: "Deliver bounded notices, failure details and recovery summaries",
	Lifetime: module.Invocation, New: newService,
}
