package preferences

import (
	"encoding/json/jsontext"

	"errors"

	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

// Source: internal/router/app_server_panes.go:19:205@8bcb5cbf nativePanePersistence

type Service struct{ history *history.Service }

func New(history *history.Service) (*Service, error) {
	if history == nil {
		return nil, errors.New("session preferences require session history")
	}
	return &Service{history: history}, nil
}

type Scope struct{ Workspace, Thread string }

// Snapshot owns settings meaning while leaving presentation schema to its
// renderer. The historical flat version-1 record identifies the terminal renderer.
type Snapshot struct {
	Settings       Applied        `json:"settings,omitzero"`
	ResumeObserved time.Time      `json:"resumeObserved,omitzero"`
	ResumeEvidence Evidence       `json:"resumeEvidence,omitzero"`
	Presentation   jsontext.Value `json:",embed"`
}

// Attachment has one write target and is owned by the attached session's
// serialized presentation owner. It starts no goroutine and opens no file.
type Attachment struct {
}

// Write is one immutable publication prepared by the selected preference owner.
// Callers serialize publications; acquiring it does not perform storage I/O.
type Write struct {
}
