package binding

import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"

// Bindings declares existing Codex connections. Factories receive the constructed
// service during registration; reference collection never invokes them.
type Bindings[S any] struct {
	Observation func(S) Observation
	Frontends   bool // Exposes the descriptor's command contributions.

	MCP func(S) MCPServer
}

func (b Bindings[S]) Reference() module.BindingReference {
	r := module.BindingReference{}
	for _, connection := range []struct {
		mechanism string
		declared  bool
	}{
		{"executable-frontend", b.Frontends},
		{"observation", b.Observation != nil},
		{"mcp-tools", b.MCP != nil},
	} {
		if connection.declared {
			r.Mechanisms = append(r.Mechanisms, connection.mechanism)
		}
	}
	return r
}
