package livepreview

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

type Deps struct {
	ReadEndpoint func(string, *int) capture.File
}

var Module = module.Descriptor[Deps, *preview.Service, module.NoBindings, module.NoViews]{
	ID: "livepreview", Summary: "Project provisional source changes and publish bounded live view state",
	Requires: []module.ID{"changehistory"}, Lifetime: module.AttachedSession,
	New: func(deps Deps, _ module.Slots) (*preview.Service, error) { return preview.New(deps.ReadEndpoint), nil },
}
