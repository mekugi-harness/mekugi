package appserver

import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"

// FeatureUse is a detached native snapshot. It is never checkpointed or incremented in Go.
type FeatureUse = observation.FeatureUse
