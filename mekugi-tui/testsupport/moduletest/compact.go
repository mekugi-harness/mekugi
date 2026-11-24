package moduletest

import (
	"context"
	json "encoding/json/v2"
	"errors"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

// StoreCompact creates a historical summary record for compatibility fixtures.
func StoreCompact(ctx context.Context, store changes.Scope, thread, summary, reload string) (string, error) {
	if store.Store == nil || thread == "" || strings.TrimSpace(summary) == "" {
		return "", errors.New("compaction publication requires a retained thread and summary")
	}
	namespace, release, err := store.Store.BeginSession(ctx, store.Writes, store.Limits, thread)
	if err != nil {
		return "", err
	}
	defer release()
	var reference string
	err = store.Store.Transaction(ctx, store.Writes, func(tx *history.Transaction) error {
		handles, err := tx.ReserveHandles(namespace, 1)
		if err != nil {
			return err
		}
		reference = history.CompactSummaryPrefix + namespace + ":" + handles[0]
		data, err := json.Marshal(history.CompactRecord{Version: 1, Reference: reference, Summary: summary, Selection: reload})
		if err != nil {
			return err
		}
		name := history.CompactSummaryName(reference)
		if err := tx.Admit(store.Limits, name, int64(len(data))); err != nil {
			return err
		}
		if err := tx.Write(name, "compact-pending-", data); err != nil {
			return err
		}
		return tx.Retain(thread, store.Limits, name)
	})
	if err != nil {
		return "", err
	}
	return reference, nil
}
