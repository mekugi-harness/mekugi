package executionobservation

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

type storageSessionKey struct{}

type storageSessionIdentity struct {
	Thread, Namespace string
}

func observationScope(ctx context.Context, selected changes.Scope) changes.Scope {
	if identity, ok := ctx.Value(storageSessionKey{}).(storageSessionIdentity); ok {
		selected.Thread, selected.Namespace = identity.Thread, identity.Namespace
	}
	return selected
}
