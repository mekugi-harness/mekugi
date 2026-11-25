// Source: apps/mekugi/internal/sessionpresentation/status.go:1:310@[14a6d1f3]
package sessionpresentation

import (
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/presentation"
	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	noticepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/presentation"
)

// MainState retains presentation facts, never operation authority. Completion
// timing remains a duration until a consumer chooses its display grammar.
type MainState struct {
	Text    string
	Alert   bool
	Started time.Time
}

type Animation uint8

const (
	Static Animation = iota
	Pulse
	Working
)

// Label keeps full text and timing. Sanitization, previews and animation frames
// belong to the consumer, not the semantic status selection.
type WaitTiming struct {
	Since   time.Time     `json:",omitzero"`
	Elapsed time.Duration `json:",omitzero"`
}

func (w WaitTiming) Duration(at time.Time) time.Duration {
	if !w.Since.IsZero() {
		return w.Elapsed + max(0, at.Sub(w.Since))
	}
	return w.Elapsed
}

type Label struct {
	Text, Failure   string
	Alert, Running  bool
	Animation       Animation
	Started         time.Time
	Wait            WaitTiming `json:",omitzero"`
	Completion      time.Duration
	CompletionKnown bool
	FailureKnown    bool
}

type MainStatus struct {
	FeedbackRevision                   uint64
	State, Session, Composer, Feedback Label
	ComposerOverride                   bool
	Details                            [2]Label
	ShellMode                          bool
	Processes                          int
	Animating                          bool
}

// SetFeedback binds a feedback value and its detail surface together. A local
// overlay uses revision zero so painting cannot acknowledge a covered source.
func (s *MainStatus) SetFeedback(value Label, revision uint64) {
	s.Feedback, s.FeedbackRevision = value, revision
	s.Details[1] = Label{}
	if value.Alert {
		s.Details[1] = value
	}
}

// MainInputs borrows current owner state only for publication. Nothing here is
// retained, and the resulting view contains no owner pointers or capabilities.
type MainInputs struct {
	Info                   MainInfo
	DisplayedWorkspace     string
	Menu                   *MenuView
	RecordedTurn           string // Playback selection, used only without live control.
	Retry                  string
	ToolInput              func(string) string // Borrowed live Activity generation owner.
	WaitTiming             func(thread, turn string) WaitTiming
	Feedback               *noticepresentation.Model
	Conversation           *conversationmodel.Session
	ClientEditors          bool // Editor contents remain client-owned, outside this view's retained publication.
	Restoring, SideRunning bool
	BTWRunning             bool
	Paused                 bool // Borrowed native lifecycle observation, not host-turn completion.
	SideThreads            []string
	SideInterrupt          sessioncontrol.InterruptTarget
	Processes              int
}

type Caption struct {
	Model, Effort, Tier          string
	ContextTokens, ContextWindow uint64
	ContextKnown                 bool
	Items                        []presentation.Item
	Limits                       []CaptionLimit // Account rate-limit windows, shortest first.
}

// CaptionLimit is one account rate-limit window's use, such as "5h" at 12%.
type CaptionLimit struct {
	Label string
	Used  float64
}

// ProjectComposerStatus selects the composer label from published session facts
// and the current prompt/editor presentation, including client-local overlays.
func (v *View) ProjectComposerStatus(draftKept bool) {
	if v.SideThread.ID != "" && v.Displayed == v.SideThread {
		return // The side conversation publishes its own status and timing.
	}
	s := &v.Status
	s.Composer, s.ComposerOverride = s.Session, false
	if v.ReadOnly == ChildDisplayed || v.Displayed.ID != v.Main.ID {
		s.Composer, s.ComposerOverride = Label{Text: "Read-only"}, true
		return
	}
	if v.Approvals.Open {
		// A typed denial reason replaces Main's draft in the composer.
		text := "approving"
		if v.Approvals.Reason && v.PromptDraft.Request != "" && v.PromptDraft.Draft.Text != "" {
			text = "deny reason"
		}
		text += " · turn waiting"
		if draftKept && v.Approvals.Reason {
			text += " · draft kept"
		}
		s.Composer = Label{Text: text}
		s.ComposerOverride = true
	} else if call := v.Questions.Call; call != nil {
		text := "answering"
		if i := v.Questions.Index; i >= 0 && i < len(call.Questions) && call.Questions[i].Note {
			text += " · note"
		}
		if call.Sync || call.Local {
			text += " · turn waiting"
		}
		if draftKept {
			text += " · draft kept"
		}
		s.Composer = Label{Text: text}
		s.ComposerOverride = true
	} else if v.Admission.ReadOnly && v.ReadOnly != Replay {
		s.Composer, s.ComposerOverride = Label{Text: "Read-only"}, true
	}
}
