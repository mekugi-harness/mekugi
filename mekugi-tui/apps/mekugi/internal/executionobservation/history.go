package executionobservation

import (
	"encoding/json"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/activitydetail"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

// mekugiHistory adapts retained call records to the capture owner.
// Exported field names are the durable JSON schema. Unexported fields are never
// serialized; they belong to an individual observation.
type mekugiHistory struct {
	ToolName string
	PluginID string

	Script          string
	Root            string
	ExecutingThread string `json:",omitempty"`
	// Caller is the canonical agent path that issued the call; Source names
	// the edit's program when the tool name alone does not (exec_command).
	Caller           string `json:",omitempty"`
	Source           string `json:",omitempty"`
	ChangeID         string
	ReviewFiles      []diffmodel.ReviewFile
	NativePatches    []changecapture.PatchObservation `json:",omitempty"`
	ResolvedBaseline *resolvedStockBaseline           `json:",omitempty"`
	ExecObservation  *changecapture.Observation       `json:",omitempty"`
	ExecOutcome      *changecapture.Outcome           `json:",omitempty"`
	HostResults      []binding.ToolResult             `json:",omitempty"`
	CommandSegments  *history.Segments             `json:",omitempty"`
	CarrierName      string
	CarrierKind      codeModeCarrierKind
	CarrierPayload   string

	Report string
	// OutputWarning and TranslationError remain readable on retained records.
	OutputWarning        string
	TranslationError     string
	CorrelationID        string
	Attempt              int
	UpstreamItem         map[string]json.RawMessage
	ReplayCarrier        bool
	CommentaryMessageIDs []string
	AlreadySatisfied     bool

	// legacy keeps frozen inventory and coverage metadata at the retained record owner.
	legacy *history.History
	// New capture evidence is lazy and feature-owned. ReviewFiles is the frozen
	// inline input accepted from historical records and old adapter fixtures.
	reviewFiles  []evidence.Review
	sourceAnchor activitydetail.SourceAnchor

	confirmed bool
	// sequence orders calls published together; it is never durable.
	sequence uint64
}

func (h mekugiHistory) reviews() []evidence.Review {
	if h.reviewFiles != nil {
		return h.reviewFiles
	}
	return evidence.InlineReviews(h.ReviewFiles)
}
