package executionobservation

import (
	"context"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/execscope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

// storeChangeResolver reads the calling thread's change namespace.
func storeChangeResolver(ctx context.Context, selected changes.Scope, source changes.Service) execscope.ChangeResolver {
	return func(arguments []string, directory string, deadline time.Time) execscope.ChangeResolution {
		options, err := source.Parse(arguments, directory)
		if err != nil {
			return execscope.ChangeResolution{InvalidArguments: true}
		}
		ctx, cancel := context.WithDeadline(ctx, deadline)
		defer cancel()
		paths, err := source.MutationPaths(ctx, observationScope(ctx, selected), options)
		if err != nil {
			return execscope.ChangeResolution{}
		}
		return execscope.ChangeResolution{Paths: paths}
	}
}
