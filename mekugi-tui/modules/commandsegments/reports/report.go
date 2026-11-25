// Package reports owns completed command-segment evidence, not live processes.
package reports

import (
	"time"
)

// Source: internal/router/command_segments.go:21:38@[cc2524a0] retained reports and identity.
type Report struct {
	Command string
	Exit    int
	Output  [32]byte // Digest of the exact host aggregate, without a second copy.
	Parts   []Part
}

type Part struct {
	Timing  Timing `json:",omitzero"`
	Source  string
	Skipped bool
	Exit    int
	Output  *string // nil means unavailable; an empty string is complete empty output.
}

type Timing struct {
	Started   time.Time `json:"started,omitzero"`
	Ended     time.Time `json:"ended,omitzero"`
	ElapsedNS int64     `json:"elapsed_ns,omitzero"`
}

// Completed contains only terminal host facts and the original ordered sources.
// The caller decodes the host's shell representation before requesting restoration.
type Completed struct {
	ID      string
	Command string
	Exit    int
	Output  [32]byte
	Sources []string
}
