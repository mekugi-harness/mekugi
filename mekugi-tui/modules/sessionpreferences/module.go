package sessionpreferences

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences/preferences"
)

var Module = module.Descriptor[Deps, *preferences.Service, module.NoBindings, module.NoViews]{
	ID: "sessionpreferences", Summary: "Retain applied settings and renderer preferences for a workspace and thread",
	Requires: []module.ID{"sessionhistory"}, Lifetime: module.Invocation,
	New: newService,
}
