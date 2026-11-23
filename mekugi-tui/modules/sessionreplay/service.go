package sessionreplay

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/replaytiming"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay/playback"
)

type Deps struct {
	History *history.Service
	Timing  replaytiming.Reader
}

func newService(deps Deps, _ module.Slots) (*playback.Service, error) {
	return playback.New(deps.History, deps.Timing)
}
