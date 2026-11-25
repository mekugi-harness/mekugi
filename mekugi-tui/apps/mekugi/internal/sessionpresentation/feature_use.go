package sessionpresentation

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
)

// FeatureReader borrows detached counts from the native launch measurement owner.
type FeatureReader interface {
	ReadFeatureUse(context.Context, []string) (map[string]observation.FeatureUse, error)
}

// FeatureSnapshot joins a native measurement with the admitted descendant set.
// Err means neither column is a complete measurement, not that it measured zero.
type FeatureSnapshot struct {
	Self, Descendants observation.FeatureUse
	Threads           []string
	Err               error
}
