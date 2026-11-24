package composition

import (
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionnaming"
)

func TestCompositionSessionNamingRemoval(t *testing.T) {
	for _, enabled := range []bool{true, false} {
		var c Composition
		if enabled {
			c.Add(sessionnaming.Module, func() sessionnaming.Deps { return sessionnaming.Deps{} })
		}
		runtime, err := c.Activate(t.Context(), new(binding.Registry))
		if err != nil {
			t.Fatal(err)
		}
		if runtime.AutoTitle != enabled {
			t.Fatalf("enabled=%v, registered=%v", enabled, runtime.AutoTitle)
		}
		if err := runtime.Close(t.Context()); err != nil {
			t.Fatal(err)
		}
	}
}
