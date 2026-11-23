package binding

import (
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
)

func TestObservationRegistration(t *testing.T) {
	var registry Registry
	if err := registry.Observation(Observation{}); err == nil {
		t.Fatal("accepted missing factory")
	}
	binding := Observation{NewTranscript: func(*scope.Store) observation.Transcript { return nil }}
	if err := registry.Observation(binding); err != nil {
		t.Fatal(err)
	}
	if err := registry.Observation(binding); err == nil {
		t.Fatal("accepted duplicate observation owner")
	}
}
