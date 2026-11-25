package sessionpresentation

import (
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

// AgentIdentity is a detached display fact. Aliases retain observed addresses,
// while Name is the canonical identity used for roster membership and navigation.
type AgentIdentity struct {
	Name    string   `json:"name"`
	Label   string   `json:"label"`
	Aliases []string `json:"aliases"`
}

// ActivityDelivery is a closed ordered set of detached terminal consequences.
// Source state is already admitted; these values cannot mutate its owners.
type ActivityDelivery struct {
	Text    *observation.TextUpdate
	Command *CommandDelivery
	Rename  *CallerRename
	Report  *CommandReportDelivery
}

type CallerRename struct{ Old, Name string }

type CommandTransition uint8

const (
	CommandBegin CommandTransition = iota + 1
	CommandDelta
	CommandComplete
	CommandRelease
)

type CommandDelivery struct {
	Kind                        CommandTransition
	Key                         [3]string
	MainSequence, ChildSequence uint64
	Text, Delta                 string
	At                          time.Time
	Tracked                     bool
	Fallback                    activitypresentation.Command
}

// CommandReportDelivery carries newly drained segment evidence to local reveal
// cursors. Snapshot contains admitted segment facts, never terminal-painted rows.
type CommandReportDelivery struct {
	Key                         [3]string
	Text                        string
	MainSequence, ChildSequence uint64
	Report                      activitypresentation.SegmentReport
	Snapshot                    activitypresentation.ReportSnapshot
	Version                     uint64
	Final                       bool
}
