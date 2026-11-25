package diagnostics

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
)

var Module = module.Descriptor[Deps, *diagnostic.Service, module.NoBindings, module.NoViews]{
	ID: "diagnostics", Summary: "Retain and inspect private diagnostic evidence",
	Requires: []module.ID{"sessionhistory"}, Lifetime: module.Invocation,
	HostsDiagnosticSections: true, New: newService,
}
