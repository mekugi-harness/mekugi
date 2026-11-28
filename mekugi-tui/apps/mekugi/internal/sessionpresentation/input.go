package sessionpresentation

import (
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

type InputAdmission struct {
	Edit, Submit bool
	ReadOnly     bool
	Reason       string
}

var ErrInputUnavailable = errors.New("Ordinary input is unavailable for this view.")

type EditorIntent struct {
	Version Version
	Target  conversationmodel.EditorTarget
	Action  composer.Action
}

func (v View) EditorTarget() conversationmodel.EditorTarget {
	if p := v.PromptDraft; p.Call != "" || p.Request != "" {
		return conversationmodel.EditorTarget{Call: p.Call, Question: p.Question, Request: p.Request, Revision: p.Revision}
	}
	return conversationmodel.EditorTarget{Revision: v.Draft.Revision}
}

// PasteIntent names a completed paste destination, never its payload.
type PasteIntent struct {
	Version Version
	Target  conversationmodel.EditorTarget
}

type ReferenceIntent struct {
	Version                   Version
	Thread                    string `json:",omitzero"`
	Displayed                 Thread
	Target                    conversationmodel.EditorTarget
	Description, Text, Source string
}

type ComposerOperation uint8

const (
	NoComposerOperation ComposerOperation = iota
	SubmitComposer
	QueueComposer
	CancelComposer
	AttachClipboard
	EditExternally
	RestoreWaiting
	IncreaseEffort
	DecreaseEffort
	AcceptPrediction
	SteerWaiting
)

type ComposerIntent struct {
	Thread    string
	Version   Version
	Target    conversationmodel.EditorTarget
	Operation ComposerOperation
	Work      *sessioncontrol.InterruptTarget `json:",omitzero"`
}
