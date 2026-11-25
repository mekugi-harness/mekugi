package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
)

func fileLinkScope(in terminalsession.FrameInput) terminalsession.SourceContext {
	if in.Selection != nil && in.Dialog == nil && in.SelectionPane == in.Shell.Layout.Main {
		return in.SelectionSource
	}
	return in.SourceContext()
}
