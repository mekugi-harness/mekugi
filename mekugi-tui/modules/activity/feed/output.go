package feed

// Output keeps a command's host output for reading in full, apart from the
// display tail rows show: its latest OutputBytes, sanitized line by line, and
// a count of the lines before them. A Retention bounds what a session keeps
// across commands. A serialized session owner reads and mutates outputs.
type Output struct {
}

// Host aggregates stop at 1 MiB; a live stream keeps as much of its latest
// output. A session keeps OutputRetainedBytes across commands.
const (
	OutputBytes         = 1 << 20
	OutputLineBytes     = 16 << 10
	outputLineOverhead  = 16 // A retained string slot, including empty lines.
	OutputRetainedBytes = 16 << 20
)

// Retention bounds the output a session keeps, releasing the oldest settled
// output first. Output still running is never released.
type Retention struct {
}

// OutputView is a read-only snapshot. Its lines are independent of later output
// mutations; consumers must not modify the published slice.
type OutputView struct {
	Lines     []string // Retained lines, then an unfinished one.
	Dropped   int      // Lines before Lines no longer retained.
	Truncated bool     // The current stream or replacement aggregate lost bytes to the line cap.
	Released  bool     // The session released this output for newer output.
	Done      bool
	Exited    bool // Exit is the host's reported status.
	Exit      int
}
