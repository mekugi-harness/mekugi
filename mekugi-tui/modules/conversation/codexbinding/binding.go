package codexbinding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

type Bindings = binding.Bindings[*presentation.Service]

var Codex = Bindings{
	Observation: func(service *presentation.Service) binding.Observation {
		return binding.Observation{NewTranscript: func(store *scope.Store) observation.Transcript {
			return service.Transcript(store)
		}}
	},
}
