// Source: apps/mekugi/internal/router/failure_store.go:1:57@b9679c93.
package requestexecution

import (
	"context"
	"errors"
	"time"

	history "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/diagnostics/diagnostic"
)

func (c *Failures) persistFailure(f Failure, d Diagnostic) {
	c.mu.Lock()
	store := c.failureStore
	limits := c.failureLimits
	writes := c.writes
	service := c.diagnostics
	c.mu.Unlock()
	if service == nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if store == nil {
		directory, err := history.ReplayDirectory()
		if err == nil {
			store, err = service.OpenFailures(ctx, directory)
		}
		if err == nil {
			c.mu.Lock()
			c.failureStore = store
			c.mu.Unlock()
		}
	}
	var err error
	if store == nil {
		err = errors.New("failure storage unavailable")
	} else {
		record := diagnostic.Failure{Version: 1, Time: time.Now().UTC(), Thread: f.Thread,
			Phase: f.Phase, Code: d.Code, Reference: d.Reference,
			Error: d.Error}
		err = service.RetainFailure(ctx, store, writes, limits, record)

	}
	if err != nil {
		c.AddThreadNotice(f.Session, f.Thread, "failure_storage", "Mekugi could not retain the failure reference. Relaunch with --debug to capture a future failure.")
	}
}
