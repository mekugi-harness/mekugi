package sessioncontrol

import (
	"context"
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

type Inspection struct {
	measurement *HostCall
	thread      string
	result      chan inspectionResult
}

type inspectionResult struct {
	details observation.SessionDetails
	err     error
}

// ReadThreadDetails observes native facts without restoring or starting the thread.
func (r *Requests[T]) ReadThreadDetails(ctx context.Context, thread string) (observation.SessionDetails, error) {
	raw, err := r.callInspection(ctx, thread, "thread/inspect", map[string]any{"threadId": thread})
	if err != nil {
		return observation.SessionDetails{}, err
	}
	info, err := binding.DecodeHistoryThread(raw, thread)
	return info.Details(), err
}

// Target identifies the requested native thread. The owning event loop validates
// Main or one of its admitted side threads before publishing any facts.
func (request *Inspection) Target() string { return request.thread }

// Inspections remains readable while a user shell blocks mutating host calls.
func (r *Requests[T]) Inspections() <-chan *Inspection { return r.inspections }

// DispatchMeasurement keeps observational RPCs on the existing ledger while
// mutating session requests remain gated by approvals or auxiliary execution.
func (r *Requests[T]) DispatchMeasurement(request *Inspection) bool {
	if request.measurement == nil {
		return false
	}
	r.Dispatch(request.measurement)
	return true
}

// Inspect borrows current facts on the runtime's serialized event loop.
func (r *Requests[T]) Inspect(ctx context.Context, thread string) (observation.SessionDetails, error) {
	request := &Inspection{thread: thread, result: make(chan inspectionResult, 1)}
	result := r.inspect(ctx, request)
	return result.details, result.err
}

func (r *Requests[T]) inspect(ctx context.Context, request *Inspection) inspectionResult {
	select {
	case r.inspections <- request:
	case <-ctx.Done():
		return inspectionResult{err: ctx.Err()}
	case <-r.closed:
		return inspectionResult{err: errors.New("runtime inspection owner closed")}
	}
	select {
	case result := <-request.result:
		return result
	case <-ctx.Done():
		return inspectionResult{err: ctx.Err()}
	case <-r.closed:
		return inspectionResult{err: errors.New("runtime inspection owner closed")}
	}
}

// Complete returns only facts belonging to the owning runtime.
func (request *Inspection) Complete(thread string, details observation.SessionDetails) {
	result := inspectionResult{details: details}
	if request.thread != thread {
		result.details = observation.SessionDetails{}
		result.err = errors.New("inspection target does not belong to this runtime")
	}
	request.result <- result
}
