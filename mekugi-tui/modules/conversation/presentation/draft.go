package presentation

import (
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

var ErrDraftChanged = errors.New("The draft changed; review it before submitting.")

type DraftView struct {
	Prediction string
	Draft      composer.Draft
	Revision   uint64
	Available  bool
}

// PromptDraftView contains only the active editor, never parked histories or secrets.
type PromptDraftView struct {
	Call, Question, Request string
	Draft                   composer.Draft
	Revision                uint64
}

type Prediction struct {
	Text     string
	Revision uint64
}

// EditorTarget distinguishes ordinary input from the current non-secret prompt.
type EditorTarget struct {
	Call, Question, Request string
	Revision                uint64
}
