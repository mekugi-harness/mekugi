// Source: apps/mekugi/internal/router/app_server_btw.go:21:377@[4ded0a4e] transient side-answer presentation.
package presentation

import (
	"time"
)

const SideCompactionRequired = "Side question requires compaction. Close this answer, compact Main, then retry."

type Answer struct{ ID, Text string }

// SideExchange is a completed display exchange, independent of host context.
type SideExchange struct {
	ID                                  uint64
	Question, AttachmentNotice, Failure string
	Answer                              []Answer
}

// SideAnswer retains bounded display history. Codex owns the full side context.
type SideAnswer struct {
	OneOff           bool
	Key              string
	Drafts           Session
	History          []SideExchange
	ID               uint64
	Question, Status string
	Pending          Input
	Answer           []Answer
	Alert, Truncated bool
	AttachmentNotice string
	Started          time.Time
}

type SideView struct {
	Key              string
	History          []SideExchange
	ID               uint64
	Question, Status string
	Answer           []Answer
	Alert, Truncated bool
	AttachmentNotice string
	Started          time.Time
}
