package launch

import (
	"os"
	"path/filepath"
	"strconv"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

func TestReplayStorageDefaultIsFourGiB(t *testing.T) {
	t.Parallel()
	store, err := openMekugiReplayStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for _, candidate := range []*mekugiReplayStore{store, testReplayStore(mekugiReplayStore{})} {
		// The reported failure needed only 170 bytes over the old 1 GiB cap.
		// Admission checks sizes, so no large payload allocation is needed.
		_, limit := candidate.publication.Usage(candidate.retentionLimits(), nil, "", (1<<30)+170)
		if limit != 4<<30 {
			t.Fatalf("storage limit = %d, want 4 GiB", limit)
		}
	}
	for number := range 33 {
		name := history.Name("/w", "existing-"+strconv.Itoa(number), false)
		file, err := os.Create(filepath.Join(store.directory, name))
		if err != nil {
			t.Fatal(err)
		}
		size := int64(maxReplayRecordBytes)
		if number == 32 {
			size = 170
		}
		if err := file.Truncate(size); err != nil {
			file.Close()
			t.Fatal(err)
		}
		if err := file.Close(); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.locked(t.Context(), func(store *mekugiReplayStore) error { return store.maintainStorage("", 1) }); err != nil {
		t.Fatalf("admission still rejects storage above the old cap: %v", err)
	}
}
