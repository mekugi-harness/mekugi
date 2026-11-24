package sessionreplay

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay/codexbinding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay/playback"
)

var Module = module.Descriptor[Deps, *playback.Service, codexbinding.Bindings, module.NoViews]{
	ID: "sessionreplay", Summary: "Schedule retained observations with synthetic streaming and read-only playback controls",
	Requires: []module.ID{"activity", "sessionhistory"}, Lifetime: module.Stateless,
	New: newService, Bindings: codexbinding.Codex,
}
