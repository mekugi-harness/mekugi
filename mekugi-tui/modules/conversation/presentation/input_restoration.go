package presentation

// InputRestoration identifies presentation changes without renderer state.
type InputRestoration struct {
	Rejected []string
	Detached []Input
	Parked   bool
	Changed  bool
}

type InputBatch struct {
	Parts  []Input
	Unsent bool
}
