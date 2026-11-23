package launch

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

// attachCommitReader keeps enrichment acquisition separate from scoped source binding.
func attachCommitReader(session *sessionpresentation.Attachment, previous *activitypresentation.CommitReader, ctx context.Context, enabled bool) *activitypresentation.CommitReader {
	state := session.Activity()
	if state.Commits == nil && enabled {
		if ctx == nil {
			ctx = context.Background()
		}
		state.Commits = activitypresentation.NewCommitReader(ctx)
	}
	if previous != state.Commits {
		previous.Close()
	}
	return state.Commits
}
