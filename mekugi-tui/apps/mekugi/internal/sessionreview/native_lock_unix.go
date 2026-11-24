//go:build unix

package sessionreview

import (
	"os"
	"path/filepath"
	"syscall"
)

func lockNativeStoreShared(dir string) (func(), error) {
	f, err := os.Open(filepath.Join(dir, "store.lock"))
	if err != nil {
		if os.IsNotExist(err) {
			return func() {}, nil
		}
		return nil, err
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_SH); err != nil {
		_ = f.Close()
		return nil, err
	}
	return func() {
		_ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
		_ = f.Close()
	}, nil
}
