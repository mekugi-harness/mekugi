// Source: apps/mekugi/internal/router/app_server_btw.go:21:377@[4ded0a4e] transient side-answer presentation.
package presentation

import (
	"slices"
	"time"
	"unicode/utf8"
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

func (s *SideAnswer) Begin(question string) {
	if s.Question != "" {
		exchange := SideExchange{ID: s.ID, Question: s.Question, Answer: s.Answer}
		if s.Alert {
			exchange.Failure = s.Status
		}
		s.History = append(s.History, exchange)
	}
	s.ID++
	s.Question, s.Status = question, "Running"
	s.Answer, s.Alert = nil, false
	s.bound()
}

// A completed native item replaces the deltas already shown for that item.
func (s *SideAnswer) Append(id, text string, complete bool) {
	index := slices.IndexFunc(s.Answer, func(item Answer) bool { return item.ID == id })
	if index < 0 {
		if len(s.Answer) >= 255 {
			s.Truncated = true
			return
		}
		s.Answer = append(s.Answer, Answer{ID: id})
		index = len(s.Answer) - 1
	}
	if complete {
		s.Answer[index].Text = text
	} else {
		s.Answer[index].Text += text
	}
	s.bound()
}

// Evict whole exchanges first, then truncate the current display at UTF-8 boundaries.
func (s *SideAnswer) bound() {
	const limit = 256 << 10
	bytes, items := len(s.Question), 1+len(s.Answer)
	for _, a := range s.Answer {
		bytes += len(a.Text)
	}
	for _, h := range s.History {
		bytes += len(h.Question) + len(h.Failure)
		items += 1 + len(h.Answer)
		for _, a := range h.Answer {
			bytes += len(a.Text)
		}
	}
	for len(s.History) > 0 && (bytes > limit || items > 256) {
		h := s.History[0]
		bytes -= len(h.Question) + len(h.Failure)
		items -= 1 + len(h.Answer)
		for _, a := range h.Answer {
			bytes -= len(a.Text)
		}
		s.History = slices.Delete(s.History, 0, 1)
		s.Truncated = true
	}
	room := limit
	retain := func(text *string) {
		if len(*text) > room {
			for room > 0 && !utf8.RuneStart((*text)[room]) {
				room--
			}
			*text = (*text)[:room]
			s.Truncated = true
		}
		room -= len(*text)
	}
	retain(&s.Question)
	for i := range s.Answer {
		retain(&s.Answer[i].Text)
	}
}
