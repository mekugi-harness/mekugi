package capture

// Source: apps/mekugi/internal/router/exec_observation.go:38:1250@1e6d58c6.

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

// Comparison supplies concurrent exclusions and an eligible workspace fallback.
// A nil Workspaces compares only the named baseline. Claims come from prior
// completed windows covering the same physical paths and Git authority.
type Comparison struct {
	Evidence   *evidence.Source
	Excluded   []string
	Workspaces *Workspaces
	Workspace  string
	Claims     Claims
	// PriorChanges means this writer already has a captured changed interval.
	// An unchanged suffix must not turn the whole writer into an unknown no-op.
	PriorChanges bool
}

// Result separates file evidence from an unavailable optional workspace comparison.
// Durable consumers retain Files and Coverage even when SnapshotError is non-nil;
// provisional consumers may keep their previous completion boundary for retry.
// Outcome holds capture facts; callers supply host status and publication references.
type Result struct {
	Outcome
	Files []evidence.Review
	// Endpoints are the named states already read for this comparison. Other
	// open writers can resume from this boundary without a later file read.
	Endpoints     []File
	Complete      bool
	Claims        Claims
	SnapshotError error
}
