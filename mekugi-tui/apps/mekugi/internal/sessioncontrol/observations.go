package sessioncontrol

import (
	"crypto/rand"
	"errors"
	"strconv"
	"strings"
	"sync"
)

// Observations coordinates invocation-local generation and control delivery.
// None of this state is restored, inherited or used to authorize execution.
// Source: apps/mekugi/internal/router/{tool_input_emission,app_server_btw,mekugi_proxy}.go@d002a0dc.
type Observations struct {
	mu                sync.RWMutex
	feedbackMu        sync.Mutex
	feedback          map[[2]string][]string
	contextIdentity   string
	contextRevision   map[string]uint64
	contextCompaction map[string]string
}

// ObserveCompactionContext consumes the request's native context-window identity,
// or a historical summary reference when that identity is unavailable. Refresh
// precedes inference independently of queued presentation events.
func (o *Observations) ObserveCompactionContext(thread, reference string) {
	if o == nil || thread == "" || reference == "" {
		return
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.contextCompaction == nil {
		o.contextCompaction = make(map[string]string)
	}
	if o.contextCompaction[thread] == reference {
		return
	}
	o.contextCompaction[thread] = reference
	if o.contextRevision == nil {
		o.contextRevision = make(map[string]uint64)
	}
	o.contextRevision[thread]++
}

// RefreshContext records an authoritative fresh/resumed/replaced context or
// completed compaction. Acquisition stays lazy until the next generating turn.
func (o *Observations) RefreshContext(thread string) {
	if o == nil || thread == "" {
		return
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.contextRevision == nil {
		o.contextRevision = make(map[string]uint64)
	}
	o.contextRevision[thread]++
}

// ContextIdentity distinguishes live history across reset, rewind and cold resume.
// The invocation identity prevents restored cursors matching a reused revision.
func (o *Observations) ContextIdentity(thread string) string {
	if o == nil || thread == "" {
		return ""
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.contextIdentity == "" {
		o.contextIdentity = rand.Text()
	}
	return o.contextIdentity + ":" + thread + ":" + strconv.FormatUint(o.contextRevision[thread], 10)
}

// ContextMatches checks the incoming native window before instruction projection
// observes it. This does not advance or replace the existing context revision.
func (o *Observations) ContextMatches(thread, identity, reference string) bool {
	if o == nil || identity == "" {
		return false
	}
	o.mu.RLock()
	defer o.mu.RUnlock()
	return identity == o.contextIdentity+":"+thread+":"+strconv.FormatUint(o.contextRevision[thread], 10) &&
		(reference == "" || reference == o.contextCompaction[thread])
}

func (o *Observations) AnswerWithFeedback(thread, turn, text string, answer func() error) error {
	if o == nil {
		return errors.New("native denial feedback requires session control")
	}
	o.feedbackMu.Lock()
	defer o.feedbackMu.Unlock()
	if err := answer(); err != nil {
		return err
	}
	if o.feedback == nil {
		o.feedback = make(map[[2]string][]string)
	}
	key := [2]string{thread, turn}
	o.feedback[key] = append(o.feedback[key], text)
	return nil
}

func (o *Observations) ClearFeedback(thread, turn string) {
	if o != nil {
		o.feedbackMu.Lock()
		delete(o.feedback, [2]string{thread, turn})
		o.feedbackMu.Unlock()
	}
}

func (o *Observations) Feedback(thread, turn string) string {
	if o == nil {
		return ""
	}
	o.feedbackMu.Lock()
	defer o.feedbackMu.Unlock()
	return strings.Join(o.feedback[[2]string{thread, turn}], "\n\n")
}
