package segments

// Source: apps/mekugi/internal/router/exec_track.go:340:476@b27b4c66.

// Subscription joins one application-owned observation window to future host
// commands. It retains report facts independently of Activity retirement.
type Subscription struct {
}

// Match is a detached report identity. Ambiguity supplies no usable evidence.
type Match struct {
	Source                   string
	Key                      [3]string
	Report                   *Report
	Ambiguous, HostCompleted bool
}

type State struct {
	Matches []Match
	Changed <-chan struct{}
}
