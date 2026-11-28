package feed

// Output keeps a command's host output for reading in full, apart from the
// display tail rows show. A serialized session owner reads and mutates outputs.
type Output struct {
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
