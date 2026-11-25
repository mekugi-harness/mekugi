package segments

// Source: apps/mekugi/internal/router/exec_track.go:29:296,562:618@b27b4c66.

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/commandsegments/transport"
)

// Codex can wait 150 ms before reporting an early command exit. Stay below
// the helper's 500 ms deadline while allowing notification delivery time.

// Observer correlates reports with host items. It neither runs commands nor
// owns the application scopes that subscribe to their observations.
type Observer struct {
}

// Report is a detached observation. HostOnly identifies a start awaiting a
// report. Ended includes transport failure; Closed and Done prove full closure.
type Report struct {
	Script                                         string
	Segments                                       []Segment
	Created                                        []transport.Message
	HostOnly, Terminal, Lossy, Ended, Closed, Done bool
	Code                                           int
	Version                                        uint64
}

type Segment struct {
	Timing       transport.Timing
	Source       string
	Began, Ended bool
	Code         int
	Output       string // Newly drained bytes, available only from Read.
}
