package binding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
)

// Observation accesses transcript correlation in application-owned scopes.
// Its callbacks consume public facts and cannot issue host operations.
type Observation struct {
	NewTranscript func(*scope.Store) observation.Transcript
}
