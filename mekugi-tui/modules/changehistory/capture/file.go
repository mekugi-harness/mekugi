package capture

// Source: apps/mekugi/internal/router/exec_observation.go:38:1250@1e6d58c6.

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"

	"os"
	"path/filepath"
	"slices"

	"unicode/utf8"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

// PathIdentities resolves parent directories, preserving a leaf symlink as its
// own filesystem entry. Missing descendants retain their nearest existing parent.
type PathIdentities map[string]string

func (p PathIdentities) directory(path string) string {
	if physical, found := p[path]; found {
		return physical
	}
	physical, err := filepath.EvalSymlinks(path)
	if err != nil {
		physical = filepath.Clean(path)
		if parent := filepath.Dir(path); errors.Is(err, os.ErrNotExist) && parent != path {
			physical = filepath.Join(p.directory(parent), filepath.Base(path))
		}
	}
	p[path] = physical
	return physical
}

// File returns the physical parent identity without following the leaf entry.
func (p PathIdentities) File(path string) string {
	return filepath.Join(p.directory(filepath.Dir(path)), filepath.Base(path))
}

func retainFile(ctx context.Context, snapshot File, budget *int, source *evidence.Source) File {
	if source == nil && budget != nil && len(snapshot.Content) > *budget {
		snapshot.Content, snapshot.Error = "", "capture budget exhausted"
	} else if source != nil && (budget == nil && len(snapshot.Content) > evidence.InlineBytes || budget != nil && len(snapshot.Content) > *budget) {
		if err := snapshot.Spill(ctx, source); err != nil {
			snapshot.Content, snapshot.Error = "", "retain baseline: "+err.Error()
		}
	} else if budget != nil {
		// Acquisition uses the collection's resident allowance. Per-record
		// chunking belongs to publication, not the filesystem read deadline:
		// an unrelated store transaction must not discard small batches.
		*budget -= len(snapshot.Content)
	}
	return snapshot
}

// execDataSnapshot is the state of a regular file with known content.
func execDataSnapshot(path string, data []byte, budget *int) File {
	snapshot := File{Path: path}
	snapshot.Kind, snapshot.Size, snapshot.Content, snapshot.Hash = execDataKind(data, budget)
	return snapshot
}

func execDataKind(data []byte, budget *int) (kind FileKind, size int64, content, hash string) {
	// NUL marks binary content, as in Git.
	if utf8.Valid(data) && !slices.Contains(data, 0) {
		if budget != nil {
			*budget -= len(data)
		}
		return Text, int64(len(data)), string(data), ""
	}
	sum := sha256.Sum256(data)
	return Binary, int64(len(data)), "", hex.EncodeToString(sum[:])
}
