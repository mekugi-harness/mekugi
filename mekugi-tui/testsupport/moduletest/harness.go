// Package moduletest constructs a module with explicit real dependencies and a
// scripted Codex binder. It is imported only by tests.
package moduletest

import (
	"context"
	"testing"


	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
)

type Scripted struct {
	binding.Registry
}

// New uses the caller's real dependencies, including temporary-storage owners.
// Composition graph and startup rollback are tested by the application owner.
func New[D, S, V any, B interface {
	module.NoBindings | binding.Bindings[S]
}](t *testing.T, descriptor module.Descriptor[D, S, B, V], deps D, slots module.Slots, script *Scripted) S {
	t.Helper()
	_ = script
	service, err := descriptor.New(deps, slots)
	if err != nil {
		t.Fatal(err)
	}
	if descriptor.Stop != nil {
		t.Cleanup(func() {
			if err := descriptor.Stop(context.WithoutCancel(t.Context()), service); err != nil {
				t.Error(err)
			}
		})
	}
	if descriptor.Start != nil {
		if err := descriptor.Start(t.Context(), service); err != nil {
			t.Fatal(err)
		}
	}
	return service
}

