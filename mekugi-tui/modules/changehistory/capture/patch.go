package capture

// Source: apps/mekugi/internal/router/native_apply_patch.go:27:511@1e6d58c6.

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

const MaxFileBytes = 8 << 20

type PatchObservation struct {
	Input string
	Files []PatchFile
}

type PatchFile = evidence.PatchFile
