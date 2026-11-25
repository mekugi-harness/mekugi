package accounting

// Source: internal/router/thread_usage_store.go@47bcdd8e retained priced totals.

import (
	"context"
)

// This is the durable form of the canonical counters, not a second usage
// calculation. Costs have already been priced per response and must not be
// recomputed with the model or tier selected at resume time.

type threadUsageWrite struct {
}

// flush waits for managed publication without holding the accounting mutex.
// Canceling a waiter does not cancel or discard the writer's pending facts.
func (u *Service) flush(ctx context.Context) error {
	u.mu.Lock()
	defer u.mu.Unlock()
	for len(u.pending) != 0 || u.writing {
		changed := u.changed
		u.mu.Unlock()
		select {
		case <-changed:
		case <-ctx.Done():
			u.mu.Lock()
			return ctx.Err()
		}
		u.mu.Lock()
	}
	return nil
}
