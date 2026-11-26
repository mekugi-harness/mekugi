package terminalview

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

// PublishReview applies the detached owner publication without opening its store.
func (c *Pane) PublishReview(view presentation.ReviewView) {
	if view.Revision == c.publicationRevision && view.Failure == c.failure && view.Workspace == c.workspace {
		return
	}
	if view.Workspace != c.workspace {
		c.Reset()
		c.SetWorkspace(view.Workspace)
	}
	c.publicationRevision, c.failure = view.Revision, view.Failure
	if view.Failure != "" {
		// A transport-capacity projection hides the pane, not its selection.
		c.dirty = true
		return
	}
	c.Update(view.Publication, view.Coverage)
}
