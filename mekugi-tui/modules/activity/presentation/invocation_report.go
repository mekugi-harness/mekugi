package presentation

import (
	"time"
)

// SegmentObservation contains admitted source and helper-observed boundaries.
// Output is a newly drained byte chunk, never a replacement aggregate.
type SegmentObservation struct {
	Source, Text, Analysis string
	Started, Ended         time.Time
	Duration               time.Duration
	Began, Finished        bool
	ExitCode               int
	Output                 string
}

// SegmentReport has no transport capability or host identity. The existing
// invocation supplies its scope; application admission supplies ordered facts.
type SegmentReport struct {
	Segments                             []SegmentObservation
	Terminal, Lossy, Ended, Closed, Done bool
	ExitCode                             int
}

type SegmentSnapshot struct {
	CommandSegment
	Raw string // Bounded original bytes for admitted VCS interpretation.
}

type ReportSnapshot struct {
	Segments                        []SegmentSnapshot
	SeparateOutput, Ended, Complete bool
	ExitCode                        int
}
