package accounting

// Source: internal/router/thread_usage.go@47bcdd8e provider accounting.

import (
	"context"
	"sync"
	"time"
)

// Thread totals are auxiliary, independent of routing sessions and replay history.
// Existing identities are never evicted: an untracked thread must not later show a
// partial lifetime total as though it were complete.
type Service struct {
	mu      sync.Mutex
	threads map[string]*threadUsageTotal
	closed  bool

	writerCtx context.Context

	// Positive native creation evidence in this router lifetime.

	pending      map[string]*threadUsageWrite
	writing      bool
	changed      chan struct{}
	writerCancel context.CancelFunc
	writerDone   chan struct{}
}

type threadUsageTotal struct {
}

func New() *Service {
	return &Service{threads: make(map[string]*threadUsageTotal), changed: make(chan struct{})}
}

func (u *Service) Close() {
	if u == nil {
		return
	}
	u.mu.Lock()
	u.closed = true
	cancel, done := u.writerCancel, u.writerDone
	u.mu.Unlock()
	if done != nil {
		ctx, stop := context.WithTimeout(context.Background(), shutdownTimeout)
		_ = u.flush(ctx) // The writer reports any unretained observations through notice.
		stop()
	}
	if cancel != nil {
		cancel()
	}
	if done != nil {
		<-done
	}
	u.mu.Lock()
	clear(u.threads)
	u.mu.Unlock()
}

// Start establishes the invocation lifetime; publication workers start on demand.
func (u *Service) Start(ctx context.Context) error {
	u.writerCtx, u.writerCancel = context.WithCancel(context.WithoutCancel(ctx))
	return nil
}

const shutdownTimeout = 5 * time.Second
