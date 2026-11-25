// Source: modules/conversation/presentation/input.go:1:179@[14a6d1f3]
// Source: apps/mekugi/internal/router/app_server_input_queue.go:35:430@[4ded0a4e] payload ordering and snapshots.
package presentation

import (
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/scope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/transcript"
)

// Service accesses only conversation-owned entries in application scope stores.
type Service struct {
	transcript scope.Key[transcript.State]
}

func (s *Service) Transcript(store *scope.Store) *transcript.State { return s.transcript.Value(store) }

// CompactIntent marks a queued command, independently of literal message text.
// Host operation gates stay in session control.
type CompactIntent struct{ WaitForIdle, Continue bool }

// Input retains immutable attachment snapshots and neutral question identity.
// It contains no host request, callback, secret, or renderer state.
type Input struct {
	composer.Draft
	// Origin identifies the requesting editor lifetime for detached restoration.
	// Empty retains the in-process editor path. It confers no execution authority.
	Origin string
	// NextTurn retains queue disposition through asynchronous recovery.
	NextTurn bool
	// Delivery identifies reserved restoration capacity at the originating view.
	// It is opaque provenance, not an execution capability or renderer owner.
	Delivery uint64
	// ShellCommand keeps queued executable input separate from model messages.
	ShellCommand string
	Compact      *CompactIntent
	Question     *prompt.Call
	ReplyCount   int
	QuestionIDs  []string
}

type Submission struct {
	composer.Draft
	Parts []Input
}

type Session struct {
	Prediction      Prediction
	Completion      completion.State
	Editor          composer.Editor
	PromptEditor    prompt.Editor
	Questions       prompt.Questions
	Approvals       prompt.Approvals
	Unsent, Queued  []Input
	Submitted       map[string]Submission
	Shell           Input
	ShellFromEditor bool // Only a consumed local draft may be restored after rejection.
	Side            *SideAnswer
	BTW             *SideAnswer
}

// ShellDraftMode applies the shared draft interpretation to a client-local
// editor without constructing a session or sharing its prompt selection.
func ShellDraftMode(text string, promptOpen bool) bool {
	return !promptOpen && strings.HasPrefix(text, "!")
}

// WaitingReason describes admission without host handles or terminal hints.
type WaitingReason uint8

const (
	Waiting WaitingReason = iota
	Steering
	ShellRunning
	SendingAfterInterrupt
	RestoringAfterInterrupt
	CompactRunning
	CompactQueued
	ApplyingSettings
	RetryAttachments
)

type PendingView struct {
	CanSteer        bool
	Reason          WaitingReason
	Waiting, Queued []PendingText
}
type PendingText struct {
	Text    string
	Replies int
}
