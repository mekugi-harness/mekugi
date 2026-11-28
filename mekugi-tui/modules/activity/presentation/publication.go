package presentation

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
)

// Entry is a detached observation publication. Values are immutable until the
// owning record is replaced; mutations copy their slices before changing them.
// It carries no original host source or retained output handles.
type Entry struct {
	Facts
	Revision uint64
	Values   []Value
	Command  *ItemContent // Detached command input for terminal-only reveal reprojection.
}

func (e Entry) ActivityFacts() Facts { return e.Facts }

func (e Entry) Hidden() bool {
	return e.Suppressed || e.Kind == "question_reply" || e.Kind == "tool" && e.Text == "" && len(e.Values) == 0
}

func (e Entry) Standalone() bool {
	return !(e.Kind == "tool" && e.Text == "" && len(e.Values) == 0)
}

func (e Entry) StatusAnimating() bool {
	if !e.Live || len(e.Values) == 0 {
		return false
	}
	last := e.Values[len(e.Values)-1]
	return last.Live && (last.Kind == "summary" || last.Kind == "progress")
}

type Agent struct {
	AgentFacts
	StatusItems        []presentation.Item
	Status, LatestKind string
	Summary            []Entry
	Skills             SkillSet
}

type AgentRow struct {
	Agent Agent
	Depth int
}

type Publication struct {
	Entries     []Entry
	Roster      []AgentRow
	MainSummary []Entry // Main's borrowed feed summary, including when no roster row is present.
	LastSeq     uint64
	LineCounts  map[string]diffmodel.Counts
	NetCounts   *diffmodel.Counts
}

// CloneValues detaches mutable slices and pointer-valued facts, including flat
// original membership, so replacing an observation cannot rewrite a publication.
func CloneValues(values []Value) []Value {
	values = slices.Clone(values)
	for i := range values {
		v := &values[i]
		v.Timeouts = slices.Clone(v.Timeouts)
		v.Reads = slices.Clone(v.Reads)
		for j := range v.Reads {
			v.Reads[j].Ranges = slices.Clone(v.Reads[j].Ranges)
			v.Reads[j].RangeLines = slices.Clone(v.Reads[j].RangeLines)
		}
		v.Questions = slices.Clone(v.Questions)
		v.WaitTargets = slices.Clone(v.WaitTargets)
		v.Tail = slices.Clone(v.Tail)
		v.Changes = slices.Clone(v.Changes)
		v.Members = CloneValues(v.Members)
		v.RequestedOperations = CloneValues(v.RequestedOperations)
		if v.Results != nil {
			result := *v.Results
			v.Results = &result
		}
		if v.Hook != nil {
			hook := *v.Hook
			v.Hook = &hook
		}
	}
	return values
}
