package sessionapi

import (
	"context"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
)

type preparedRequestOwner struct {
	ctx    context.Context
	thread string
}

func (o *preparedRequestOwner) Inspect(ctx context.Context, thread string) (observation.SessionDetails, error) {
	o.ctx, o.thread = ctx, thread
	return observation.SessionDetails{}, nil
}

func TestHubSideRequestsReleaseRegistryBeforeAdmission(t *testing.T) {
	t.Parallel()
	hub := NewHub(t.Context(), nil)
	server := NewLive()
	hub.running[&managedSession{ctx: t.Context(), server: server}] = struct{}{}
	owner := new(preparedRequestOwner)
	server.BindRequests(owner)
	server.BindSideThreads(func(thread string) bool {
		// Admission must be able to query the registry, just as publication
		// queries family state while holding the server lock.
		_ = hub.activeThread("parent")
		return thread == "side"
	})
	completed := make(chan struct{})
	go func() {
		defer close(completed)
		for _, thread := range []string{"side", "unknown"} {
			requests, err := hub.Requests(thread)
			if thread == "side" && (err != nil || requests != owner) {
				t.Errorf("side admission: %v, %v", requests, err)
			}
			if thread == "unknown" && err == nil {
				t.Error("unknown thread acquired the side owner")
			}
		}
	}()
	select {
	case <-completed:
	case <-time.After(5 * time.Second):
		t.Fatal("side admission held the registry lock")
	}
}
