package sessionapi

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

type savedReview struct {
	Page      *PageCursor    `json:"page,omitzero"`
	FileCount int            `json:"fileCount"`
	Revision  uint64         `json:"revision"`
	Workspace string         `json:"workspace"`
	Coverage  string         `json:"coverage"`
	Failure   string         `json:"failure,omitzero"`
	Callers   []callerReview `json:"callers"`
	Counts    changes.Counts `json:"counts"`
}

type reviewFile struct {
	Content *ContentReference `json:"content,omitzero"`
	Key     string            `json:"key"`
	Path    string            `json:"path"`
	Status  string            `json:"status,omitzero"`
	Chunks  []changes.Chunk   `json:"chunks"`
	Origins []changes.Origin  `json:"origins"`
	Counts  diffmodel.Counts  `json:"counts"`
}

// callerReview is one caller's files as the terminal's caller filter shows
// them: other callers' captures compose as baseline, so each diff is exact.
type callerReview struct {
	Caller string `json:"caller"`
	Count  int    `json:"count"`
}

// projectReview shares the saved pane's policy and already-composed chunks.
// The server calls it only when the admitted review revision changes.

// Long paths remain fully readable without making their metadata unpageable.
