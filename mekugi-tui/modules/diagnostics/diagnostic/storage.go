// Source: internal/router/debug_storage.go@[ac8511fa] bundle provenance and retention.
package diagnostic

import (
	"errors"
	"os"
	"path/filepath"

	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"

	"github.com/gofrs/flock"
)

// This empty lease is also provenance: only bundles created by this owner have it.
const debugBundleLease = ".mekugi-debug-v1.lock"

func createBundle(root string) (string, func() error, error) {
	if err := persistence.EnsurePrivateDirectory(root); err != nil {
		return "", nil, err
	}
	directory, err := os.MkdirTemp(root, "mekugi-debug-")
	if err != nil {
		return "", nil, err
	}
	lease := flock.New(filepath.Join(directory, debugBundleLease), flock.SetPermissions(0600))
	if err := lease.Lock(); err != nil {
		return "", nil, err
	}
	return directory, func() error {
		// Inactivity starts on close, not launch. Active writers are protected by the lease.
		now := time.Now()
		return errors.Join(os.Chtimes(lease.Path(), now, now), lease.Unlock())
	}, nil
}
