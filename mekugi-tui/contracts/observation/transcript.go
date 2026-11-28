package observation

import "time"

// ItemRef preserves host identity independently of display names and feed order.
type ItemRef struct{ ThreadID, TurnID, ItemID string }

const ReasoningPart Kind = "item/reasoning/summaryPartAdded"

// TextUpdate is a transient projection into an existing feed, not another
// transcript. Sequence is assigned by the feed consumer before terminal pacing.
type TextUpdate struct {
	Ref                       ItemRef
	Replaces                  string
	Kind, Phase, Text, Status string
	Observed, Completed       time.Time
	Thought                   time.Duration
	Live                      bool
	Sequence                  uint64
}
