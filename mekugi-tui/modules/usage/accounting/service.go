package accounting

// Source: internal/router/thread_usage.go@47bcdd8e provider accounting.

import (
	"context"
	"sync"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

// Thread totals are auxiliary, independent of routing sessions and replay history.
// Existing identities are never evicted: an untracked thread must not later show a
// partial lifetime total as though it were complete.
type Service struct {
	mu            sync.Mutex
	threads       map[string]*threadUsageTotal
	closed        bool
	store         *history.Store
	writes        *persistence.Counter
	limits        history.Limits
	writerCtx     context.Context
	storageFailed map[string]bool
	fresh         map[string]bool // Positive native creation evidence in this router lifetime.
	notice        func(string, error)
	pending       map[string]*threadUsageWrite
	writing       bool
	changed       chan struct{}
	writerCancel  context.CancelFunc
	writerDone    chan struct{}
}

type threadUsageTotal struct {
	roundOutput  RoundOutput
	cost         Cost
	counts       inference.Usage
	complete     bool
	missingUsage uint64
	roundtrips   uint64
	models       []string
	priorUnknown bool
}

func New() *Service {
	return &Service{threads: make(map[string]*threadUsageTotal), changed: make(chan struct{})}
}

// ObserveResponse projects one native provider completion. Other traffic is
// not inferred from context counts or from turn lifecycle notifications.
func (u *Service) ObserveResponse(thread, model, tier string, openai bool, counts inference.Usage, duration time.Duration) {
	if u == nil || thread == "" || len(thread) > maxThreadIdentityBytes {
		return
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	if u.closed {
		return
	}
	delta := newThreadUsageTotal()
	delta.roundtrips = 1
	if !counts.TotalsKnown {
		delta.missingUsage = 1
	} else {
		delta.counts = counts
		if duration > 0 && !counts.Inconsistent {
			delta.roundOutput = RoundOutput{
				StartedUnixNano: time.Now().Add(-duration).UnixNano(),
				Throughput:      inference.OutputThroughput{OutputTokens: counts.OutputTokens, DurationNanos: uint64(duration), MeasuredRequests: 1},
			}
		}
		if openai {
			delta.cost = estimateReferenceCost(model, tier, counts)
		} else {
			delta.cost.Known = false
		}
	}
	u.updateLocked(thread, delta)
}

func (u *Service) Snapshot(thread string) Report {
	if u == nil {
		return Report{}
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	if u.closed {
		return Report{}
	}
	u.loadLocked(thread)
	total := u.threads[thread]
	if total == nil {
		return Report{}
	}
	report := Report{PriorUnknown: total.priorUnknown, Roundtrips: total.roundtrips, OutputThroughput: total.roundOutput.Throughput}
	if total.complete {
		report.Usage = total.counts
		report.TotalsKnown = total.roundtrips > total.missingUsage
		report.MissingUsage = total.missingUsage
		report.Cost, report.CostKnown = total.cost.Total(), report.TotalsKnown && total.cost.Known
	}
	return report
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

// Configure attaches the selected publication boundary before observations arrive.
func (u *Service) Configure(store *history.Store, writes *persistence.Counter, limits history.Limits, notice func(string, error)) {
	if u == nil {
		return
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	u.store = store
	u.writes, u.limits, u.notice = writes, limits, notice
}

const maxThreadIdentityBytes = 16 << 10
const shutdownTimeout = 5 * time.Second
