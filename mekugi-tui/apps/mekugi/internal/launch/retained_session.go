package launch

import (
	"context"
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"

	history "github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func openRetainedSession(ctx context.Context, service *history.Service, retained *history.Session, thread string, store *history.Store, writes *persistence.Counter, limits history.Limits) error {
	retained.Close()
	if ctx == nil {
		ctx = context.Background()
	}
	if store == nil {
		directory, err := history.ReplayDirectory()
		if err != nil {
			return err
		}
		store, err = service.Open(ctx, directory)
		limits = history.Limits{Total: history.DefaultStorageBytes, Kinds: map[string]int64{"commentary": 16 << 20}}
		if err != nil {
			return err
		}
	}
	selected, err := store.SelectSession(ctx, writes, limits, thread)
	if publication, ok := errors.AsType[*history.PublicationError](err); ok {
		err = requestexecution.StoragePublicationError(publication.Err)
	}
	if err != nil {
		return err
	}
	*retained = selected
	return nil
}
