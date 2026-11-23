package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
)

// Late metadata updates canonical labels before detached terminal delivery.
func renameThreadActivity(attached *attachedPresentation, old, name string) {
	attached.presentation.RenameCaller(old, name)
	for _, source := range []*sessionpresentation.ActivitySource{attached.source, attached.childSource} {
		source.RenameCaller(old, name)
	}
}
