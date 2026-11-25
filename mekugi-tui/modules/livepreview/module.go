package livepreview

import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"

type Deps struct{}

type Service struct{}

var Module = module.Descriptor[Deps, *Service, module.NoBindings, module.NoViews]{
	ID: "livepreview", Summary: "Live preview view types and terminal renderer",
	Lifetime: module.Stateless,
	New:      func(Deps, module.Slots) (*Service, error) { return &Service{}, nil },
}
