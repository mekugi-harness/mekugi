package capture

// CommandOutcome is admitted host evidence for one nested command, not a
// conclusion inferred from source or the outer cell's result.
type CommandOutcome struct {
	Completed bool
	Exit      *int
}

type Completion struct {
	CodeMode, Succeeded bool
	Exit                *int
	Confirmed           bool
	Commands            []CommandOutcome
}
