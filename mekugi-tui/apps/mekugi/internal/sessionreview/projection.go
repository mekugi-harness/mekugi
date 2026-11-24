package sessionreview

import (
	"context"
	"errors"
	"maps"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"

	previewmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

// Projection hydrates admitted retained evidence for Diff and Activity consumers.
// It owns no terminal interaction, renderer or capture lifecycle.
// Source: apps/mekugi/internal/router/live_diff_terminal.go:315:378@[fc0514ff]
// Source: apps/mekugi/internal/router/saved_diff.go:17:79@[bab31cbe] savedDiff.
type Projection struct {
	Changes        changes.Service
	Retained       changes.Scope
	NativeDir      string
	Callers        map[string]string
	Data           *presentation.Snapshot
	Scope          previewmodel.Scope
	Workspace      string
	Coverage       string
	awaitingResync bool
}

func (c *Projection) Reset() {
	c.Data, c.Scope, c.Coverage = new(presentation.Snapshot), previewmodel.Scope{}, "CONNECTING"
	c.NativeDir, c.Callers = "", nil
	c.awaitingResync = true
}

func (c *Projection) Apply(ctx context.Context, events ...Event) (bool, error) {
	next := *c
	builder := presentation.NewBuilder(c.Data)
	changed, content := false, false
	for _, event := range events {
		if err := ctx.Err(); err != nil {
			return false, err
		}
		if err := validateEvent(event); err != nil {
			return false, err
		}
		if next.awaitingResync {
			if event.Kind != "scope" || !event.Resync {
				continue
			}
			next.awaitingResync = false
		}
		switch event.Kind {
		case "scope":
			if event.NativeDir != "" || event.Resync {
				next.NativeDir = event.NativeDir
			}
			if event.Callers != nil {
				next.Callers = maps.Clone(event.Callers)
			}
			if next.Retained.Store != nil || next.NativeDir != "" {
				if err := reconcileReview(ctx, next.Retained, next.Changes, next.NativeDir, next.Callers, next.Data, builder, *event.Scope); err != nil {
					return false, err
				}
			}
			next.Scope, next.Coverage = *event.Scope, event.Status
			changed, content = true, true
		case "native":
			if event.NativeDir != "" {
				next.NativeDir = event.NativeDir
			}
			if event.Callers != nil {
				next.Callers = maps.Clone(event.Callers)
			}
			if next.NativeDir != "" {
				legacyCalls := collectLegacyCalls(next.Data)
				added, err := reconcileNativeReview(ctx, next.NativeDir, next.Callers, builder, next.Scope, legacyCalls, nil)
				if err != nil {
					return false, err
				}
				if added {
					changed, content = true, true
				}
			}
		case "change":
			for _, change := range event.Changes {
				if !next.Scope.Workspaces[change.Workspace][change.Thread] {
					continue
				}
				if next.Retained.Store == nil {
					return false, errors.New("saved change evidence is unavailable")
				}
				if err := hydrateReview(ctx, next.Retained, builder, change, nil); err != nil {
					return false, err
				}
			}
			changed, content = true, true
		}
	}
	if content {
		next.Data = builder.Finish()
	}
	*c = next
	return changed, nil
}
