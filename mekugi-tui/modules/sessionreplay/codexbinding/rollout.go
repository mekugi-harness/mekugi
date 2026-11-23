package codexbinding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionreplay/playback"
)

type Bindings = binding.Bindings[*playback.Service]

var Codex = Bindings{
	Rollout: func(service *playback.Service, source binding.RolloutSource) error {
		return service.BindSource(playback.Decoder{Read: source.Replay, Identity: source.Identity})
	},
}
