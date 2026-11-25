package binding

type DispatchOutcome string

const (
	NotDispatched DispatchOutcome = "not_dispatched"
	HostRejected  DispatchOutcome = "rejected"
	Uncertain     DispatchOutcome = "uncertain"
)

// DispatchError distinguishes an explicit host rejection from a lost
// acknowledgement. Uncertain effects must be reconciled before another dispatch.
type DispatchError struct {
	Outcome DispatchOutcome
	Method  string
	Cause   error
}
