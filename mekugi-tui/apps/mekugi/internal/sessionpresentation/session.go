// Package sessionpresentation owns renderer-neutral attached-session display and
// observational history state. It never stores transcript content or sends RPCs.
package sessionpresentation

import (
	"errors"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"

	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
)

type Thread struct {
	ID     string
	Caller string
}

type ReadOnly uint8

const (
	Editable ReadOnly = iota
	ChildDisplayed
	Replay
)

type Version struct {
	Attachment uint64
	Revision   uint64
}

// SameAttachment admits an observed revision of the current attachment. The
// operation owner must separately validate its destination and current state.
func (v Version) SameAttachment(current Version) bool {
	return v.Attachment == current.Attachment && v.Revision <= current.Revision
}

type MainInfo struct {
	Skills                                           activitypresentation.SkillSet
	Workspace, Title, BackendVersion, ApprovalCaller string
	Caller                                           string
}

type View struct {
	DisplayedWorkspace string
	Info               MainInfo
	Menu               *MenuView
	Resume             *ResumeView
	Version            Version
	Main               Thread
	Displayed          Thread
	Input              Thread
	ReadOnly           ReadOnly
	Loading            string
	Status             MainStatus
	Caption            Caption
	Questions          prompt.QuestionDockView
	Approvals          prompt.ApprovalDockView
	Pending            conversationmodel.PendingView
	Draft              conversationmodel.DraftView
	PromptDraft        conversationmodel.PromptDraftView
	Admission          InputAdmission
	Interrupt          sessioncontrol.InterruptTarget
	InterruptAvailable bool
	Side               *SideToken
	BTW                *conversationmodel.SideView
	BTWRunning         bool
	SideThread         Thread
	Panel              *PanelView
}

// SourceWorkspace names displayed content without changing Main's input authority.
func (v View) SourceWorkspace() string {
	if v.DisplayedWorkspace != "" {
		return v.DisplayedWorkspace
	}
	return v.Info.Workspace
}

type DisplayIntent struct {
	Version Version
	Thread  Thread
}

var ErrStaleIntent = errors.New("The action's original session or target is unavailable.")
var ErrReplayIntent = errors.New("Replay is read-only.")

// Service owns entry keys and attachment identity. The application owns stores.
type Service struct {
}

// Source: apps/mekugi/internal/router/native_shell.go:265:283@db279442
type Model struct {
	Status MainState
}
