package binding

// ExecutionResult describes the host envelope, not the success of any nested
// tool. Consumers join native trace evidence before assigning nested outcomes.
type ExecutionResult struct {
	Terminal  bool
	Succeeded bool
	ExitCode  *int
	Text      string
}
