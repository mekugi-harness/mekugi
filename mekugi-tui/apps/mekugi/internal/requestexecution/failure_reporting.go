package requestexecution

import (
	"crypto/hmac"
	"crypto/sha256"
	"errors"
	"fmt"

	responseevents "github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/notice"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

// FailureStorage borrows the composed publication and diagnostic owners.
type FailureStorage struct {
	Diagnostics *diagnostic.Service
	Store       *history.Store
	Limits      history.Limits
	Writes      *persistence.Counter
}

func (c *Failures) Configure(storage FailureStorage) {
	if c == nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.diagnostics, c.failureStore, c.failureLimits, c.writes = storage.Diagnostics, storage.Store, storage.Limits, storage.Writes
}

// Diagnostic contains the published classification and private reference.
type Diagnostic struct{ Code, Reference, Error, Message string }

// Failure is an already-failed operation, detached from an attempt.
type Failure struct {
	Session, Thread, Turn, Phase string
	UpstreamStatus               int
	Terminal                     responseevents.TerminalState
}

func (c *Failures) Report(failure Failure, err error) Diagnostic {
	if c == nil || err == nil {
		return Diagnostic{}
	}
	mac := hmac.New(sha256.New, []byte(c.diagnosticSalt))
	fmt.Fprintf(mac, "%s|%d|%s|%s", failure.Phase, failure.UpstreamStatus, failure.Terminal, err)
	reference := fmt.Sprintf("%x", mac.Sum(nil)[:6])
	code, message := "unclassified", err.Error()
	if d, ok := errors.AsType[*DiagnosticError](err); ok {
		code = d.Code
	}
	message += " Diagnostic reference: " + reference + "."
	category := failure.Phase + ":" + code + ":" + reference
	if failure.Turn != "" {
		category += ":" + failure.Thread + ":" + failure.Turn
	}
	c.Notices.Submit(notice.Input{
		ID:      noticeID("critical:" + failure.Session + ":" + category),
		Session: failure.Session, Thread: failure.Thread, Turn: failure.Turn,
		Category: category, Reference: reference, Text: message, Request: true,
	})
	detail := Diagnostic{Code: code, Reference: reference, Error: err.Error(), Message: message}
	c.persistFailure(failure, detail)
	return detail
}
