// Package persistence owns retained-file admission, application-write accounting
// and atomic publication.
// It leaves flushing to the kernel; successful publication is not a power-loss guarantee.
package persistence

import (
	"io"
	"os"
	"path/filepath"
)

// AtomicFile closes a private same-directory temporary file before rename.
func AtomicFile(path, pattern string, data []byte) error {
	return atomicFileMode(path, pattern, data, 0)
}

func atomicFileMode(path, pattern string, data []byte, mode os.FileMode) error {
	f, err := os.CreateTemp(filepath.Dir(path), pattern)
	if err != nil {
		return err
	}
	defer func() { f.Close(); os.Remove(f.Name()) }()
	n, err := f.Write(data)
	if err != nil {
		return err
	}
	if n != len(data) {
		return io.ErrShortWrite
	}
	if mode != 0 {
		if err := f.Chmod(mode); err != nil {
			return err
		}
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), path)
}
