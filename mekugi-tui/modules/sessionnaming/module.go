package sessionnaming

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionnaming/codexbinding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionnaming/internal/naming"
)

var Module = module.Descriptor[Deps, naming.Service, codexbinding.Bindings, module.NoViews]{
	ID: "sessionnaming", Summary: "Generate optional thread names while preserving manual authority", Lifetime: module.HostOperation,
	New: newService, Bindings: codexbinding.Codex,
}
