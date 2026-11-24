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

func (s TerminalState) String() string {
	switch s {
	case TerminalInvalid:
		return "invalid"
	case TerminalPending:
		return "pending"
	case TerminalCompleted:
		return "completed"
	case TerminalSteered:
		return "steered"
	case TerminalFailed:
		return "failed"
	case TerminalInterrupted:
		return "interrupted"
	default:
		return "unknown"
	}
}
