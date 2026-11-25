package presentation

import (
	"time"
)

// CommandSource is authenticated display source, not executable input. The
// backend adapter removes its own instrumentation and supplies the directory.
type CommandSource struct{ Script, Display, Directory string }

type CommandCompletion struct {
	Source    CommandSource
	At        time.Time
	Duration  time.Duration
	Aggregate *string
	Exit      *int
}

// Invocation owns one admitted host command, independently of turn lifetime and
// terminal pacing. The application's operation index and retained feed records
// reference this same object; it has no registry, process or storage capability.
type Invocation struct {
}
