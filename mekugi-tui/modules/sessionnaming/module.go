package sessionnaming

import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"

type Deps struct{}

var Module = module.Descriptor[Deps, struct{}, module.NoBindings, module.NoViews]{
	ID: "sessionnaming", Summary: "Generate optional thread names while preserving manual authority", Lifetime: module.Stateless,
	New: func(Deps, module.Slots) (struct{}, error) { return struct{}{}, nil },
}
