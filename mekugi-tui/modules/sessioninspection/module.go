package sessioninspection

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessioninspection/codexbinding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessioninspection/query"
)

var Module = module.Descriptor[Deps, *query.Service, codexbinding.Bindings, module.NoViews]{
	ID: "sessioninspection", Summary: "Query retained calls, outcomes and static corpus findings",
	Requires: []module.ID{"sessionhistory", "changehistory"}, Lifetime: module.Invocation, HostsInspection: true,
	New: newService, Bindings: codexbinding.Codex,
}
