package codexbinding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
)

type Bindings = binding.Bindings[*capture.Service]

var Codex = Bindings{
	Frontends: true,
}
