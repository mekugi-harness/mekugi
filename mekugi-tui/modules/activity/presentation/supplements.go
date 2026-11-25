package presentation

import (
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/activitydetail"
)

const SupplementMessageBytes = 64 << 10

// SupplementEditReceipt annotates a retained host call, never a new tool event.
// Calls identifies the original host items whose captured effects Text describes.
const SupplementEditReceipt = "edit_receipt"

// ReloadSource is an orchestration-issued attachment intent. Host events own its lifecycle.
type ReloadSource struct {
	Command, Path, Skill string
	Changes              string `json:",omitzero"`
	ExitCode             *int   // Present only after an authenticated per-source result.
	Preload              bool   `json:",omitzero"`
}
type Supplement struct {
	SourceAnchor                       activitydetail.SourceAnchor
	ReloadSources                      []ReloadSource
	Thread, Source, Kind, Text, CallID string
	Turn                               string
	Calls                              []string // Ordered host identities for an edit receipt.
	Observed                           time.Time
}
type RequestStart struct {
	Thread string
	At     time.Time
}
type Unreturned struct{ Thread, Call string }

// Audience is a detached application-authenticated routing snapshot, not a roster.
type Audience struct {
	Root    string
	Threads map[string]string
}

// Supplements owns invocation-scoped auxiliary observations. Its application
// serializes access together with the supplied thread scopes and authority facts.
// Source: apps/mekugi/internal/router/subagent_activity{,_pane}.go@d002a0dc.
type Supplements struct {
}
