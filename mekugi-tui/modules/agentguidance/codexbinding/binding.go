package codexbinding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentguidance/compact"
)

type Bindings = binding.Bindings[*compact.CaptureService]

var Codex = Bindings{Frontends: true}
