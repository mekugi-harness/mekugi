package capture

// Source: apps/mekugi/internal/router/native_apply_patch.go:27:511@1e6d58c6.

import (
	"errors"
	"os"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/sourceanalysis/sourceparse"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

const MaxFileBytes = 8 << 20

type PatchObservation struct {
	Input string
	Files []PatchFile
}

type PatchFile = evidence.PatchFile

func ReadPatchFile(path string) (content string, exists bool, err error) {
	if path == "" {
		return "", false, nil
	}
	file, err := sourceparse.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return "", false, nil
	}
	if err != nil {
		return "", false, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return "", false, err
	}
	if !info.Mode().IsRegular() || info.Size() > MaxFileBytes {
		return "", true, errors.New("file is not a bounded regular file")
	}
	content, err = sourceparse.ReadText(file, MaxFileBytes, "file exceeds capture capacity or is not UTF-8")
	return content, true, err
}
