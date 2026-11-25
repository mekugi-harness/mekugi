package responses

// TerminalState is validated response evidence. It is not request success.
type TerminalState uint8

const (
	TerminalUnknown TerminalState = iota
	TerminalInvalid
	TerminalPending
	TerminalCompleted
	TerminalSteered
	TerminalFailed
	TerminalInterrupted
)
