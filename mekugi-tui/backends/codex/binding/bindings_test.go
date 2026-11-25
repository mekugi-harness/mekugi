package binding

import (
	"slices"
	"testing"
)

func TestBindingsReferenceListsDeclaredMechanisms(t *testing.T) {
	ref := (Bindings[struct{}]{
		Frontends:   true,
		Observation: func(struct{}) Observation { return Observation{} },
		MCP:         func(struct{}) MCPServer { return MCPServer{} },
	}).Reference()
	want := []string{"executable-frontend", "observation", "mcp-tools"}
	if !slices.Equal(ref.Mechanisms, want) {
		t.Fatalf("Reference().Mechanisms = %v, want %v", ref.Mechanisms, want)
	}
}
