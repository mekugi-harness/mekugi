// Package binding exposes the Codex mechanisms implemented during extraction.
package binding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/command"
)

type Registrar interface {
	MCP(MCPServer) error
	Observation(Observation) error
	Frontend(command.Spec) error
}

// Registry validates and retains composition-ordered Codex connections.
type Registry struct {
}
