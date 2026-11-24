// Source: apps/mekugi/internal/router/critical_errors.go:1:316@b9679c93.
package requestexecution

import (
	"crypto/rand"
	"crypto/sha256"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"sync"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
)

// Failures adapts request classification and private failure publication
// to an independently optional notice owner. It holds no presentation state.
type Failures struct {
	Notices        *notice.Service
	diagnostics    *diagnostic.Service
	failureLimits  history.Limits
	writes         *persistence.Counter
	mu             sync.Mutex
	diagnosticSalt string
	failureStore   *history.Store
}

func NewFailures(notices *notice.Service) *Failures {
	return &Failures{Notices: notices, diagnosticSalt: rand.Text()}
}

// DiagnosticError keeps a stable classification alongside the underlying
// error. Notices and failure records include the complete error string.
type DiagnosticError struct {
	Cause   error
	Code    string
	Summary string
}

func (e *DiagnosticError) Error() string {
	return e.Cause.Error()
}
func (e *DiagnosticError) Unwrap() error { return e.Cause }

func Classify(err error, code, summary string) error {
	if err == nil {
		return nil
	}
	if _, ok := errors.AsType[*DiagnosticError](err); ok {
		return err
	}
	return &DiagnosticError{Cause: err, Code: code, Summary: summary}
}

func (c *Failures) ObserveTurnCompletion(method string, params []byte) {
	if c == nil || method != "turn/completed" {
		return
	}
	var event binding.Notification
	if json.Unmarshal(params, &event) != nil {
		return
	}
	c.Notices.ObserveTurn(event.ThreadID, event.Turn.ID, event.Turn.Status)
}

func (c *Failures) AddNotice(session, category, message string) {
	c.AddThreadNotice(session, "", category, message)
}

func (c *Failures) AddThreadNotice(session, thread, category, message string) {
	if c == nil {
		return
	}
	c.Notices.Submit(notice.Input{ID: noticeID("notice:" + session + ":" + thread + ":" + category), Session: session, Thread: thread, Category: category, Text: message})
}

// AddThreadEvent reports an informational thread occurrence as transcript
// progress, never as composer error feedback.
func (c *Failures) AddThreadEvent(session, thread, category, message string) {
	if c == nil {
		return
	}
	c.Notices.Submit(notice.Input{ID: noticeID("notice:" + session + ":" + thread + ":" + category), Session: session, Thread: thread, Category: category, Text: message, Surface: notice.Progress})
}

// Cleanup supplies a typed surface at the producer boundary, rather than asking
// a renderer to infer presentation from a category string.
func (c *Failures) CleanupNotice(session, thread, phase, message string) {
	if c == nil {
		return
	}
	category := "storage_cleanup_" + phase
	surface := notice.Error
	if phase == "planning" || phase == "reclaimed" {
		surface = notice.Progress
	}
	c.Notices.Submit(notice.Input{ID: noticeID("notice:" + session + ":" + thread + ":" + category), Session: session, Thread: thread, Category: category, Text: message, Surface: surface})
}

func noticeID(seed string) string {
	digest := sha256.Sum256([]byte(seed))
	return fmt.Sprintf("msg_mekugi_commentary_%x", digest[:12])
}
