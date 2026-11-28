package sessionapi

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

// Family is shared navigation, independent of the selected runtime's transcript.
// Threads includes attachable admitted cold targets, but never history-only roster rows.
type Family struct {
	Questions map[string][]prompt.CallView `json:"-"`
	Live      []preview.Preview            `json:"-"`
	LiveScope preview.Scope                `json:"-"`
	Root      string                       `json:"root"`
	Revision  uint64                       `json:"revision,omitzero"`
	Threads   []string                     `json:"threads"`
	Agents    *activity.Publication        `json:"agents"`
	Status    sessionpresentation.Label    `json:"status"`
}

// Family borrows ordinary raw publications in the selected logical family.
// A runtime's lifetime parent does not grant membership after thread switching.

// ProgressSnapshot borrows the immutable family capture publication without
// renderer Diff filtering. Consumers must not mutate the returned snapshot.

// Retain only active logical families across the admitted runtime registry.
// One revision sequence prevents identity reuse when a family is replaced.
