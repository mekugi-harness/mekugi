package launch

import (
	"crypto/sha256"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
)

func usageStoreFixture(t *testing.T, directory string) *mekugiReplayStore {
	t.Helper()
	store, err := openMekugiReplayStore(directory)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(store.snapshots.Close)
	return store
}
func storedUsageFixture(t *testing.T, store *mekugiReplayStore) *accounting.Service {
	u := accounting.New()
	t.Helper()
	if err := u.Start(t.Context()); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(u.Close)
	u.Configure(store.publication, nil, history.Limits{Total: history.DefaultStorageBytes}, nil)
	return u
}

func writeLegacyUsageFixture(t *testing.T, directory, thread, fields string) {
	t.Helper()
	path := filepath.Join(directory, fmt.Sprintf("usage-%x.json", sha256.Sum256([]byte(thread))))
	data := fmt.Sprintf(`{"Version":1,"Thread":%q,%s}`, thread, fields)
	if err := os.WriteFile(path, []byte(data), 0600); err != nil {
		t.Fatal(err)
	}
}
