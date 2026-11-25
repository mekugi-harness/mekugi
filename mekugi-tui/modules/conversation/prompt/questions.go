// Source: apps/mekugi/internal/router/app_server_questions.go:25:758@[42afca9c] question presentation and lifecycle.
package prompt

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
)

type Option struct {
	Label, Description, Preview string
	Recommended                 bool
}

// Question contains editable non-secret state. Secret editors and answers stay
// with the adapter that owns the authentication request.
type Question struct {
	ID, Header, Question string
	IsSecret             bool
	Choices              []Option
	Editor               composer.State
	Selected             int
	Note, Done, Skipped  bool
	Answer               []string
	Outcome              string
	Sent                 bool
	SubmissionID         string
}

type Call struct {
	ID, Thread, Turn, Item string
	Sync, Local            bool
	Questions              []Question
	Sent, Resolved         bool
	SubmissionID           string
	SubmitIDs              []string
}

// Questions belongs to one attached session. Adapters retain request handles by
// Call identity; the model never receives a host client or a live handle.
type Questions struct {
	Calls             []*Call
	Active            *Call
	Index             int
	Confirm, AutoOpen bool
}

// threadPending keeps the thread's unanswered calls, plus local ones.
func threadPending(thread string) func(*Call) bool {
	return func(c *Call) bool { return (c.Thread == thread || c.Local) && !c.Resolved && !c.Sent }
}

func (m *Questions) Next() *Call {
	for _, c := range m.Calls {
		if (c.Sync || c.Local) && !c.Resolved && !c.Sent {
			return c
		}
	}
	for _, c := range m.Calls {
		if !c.Resolved && !c.Sent {
			return c
		}
	}
	return nil
}
func (m *Questions) Open(c *Call) bool {
	if m.Active != nil || c == nil {
		return false
	}
	m.Active, m.Index, m.Confirm = c, 0, false
	for i, q := range c.Questions {
		if q.Outcome == "" && !q.Sent {
			m.Index = i
			break
		}
	}
	return true
}
func (m *Questions) Hide() { m.Active = nil; m.Confirm = false }
func (m *Questions) Current() *Question {
	if m.Active == nil {
		return nil
	}
	return &m.Active.Questions[m.Index]
}
func (m *Questions) Move(step int) {
	m.Index = (m.Index + step + len(m.Active.Questions)) % len(m.Active.Questions)
	m.Confirm = false
}
func (m *Questions) ShouldOpen(vacant bool) bool {
	return m.AutoOpen && m.Active == nil && vacant && m.Next() != nil
}

func (q *Question) Choose(index int) { q.Selected = index; q.Done = false }
func (q *Question) Edit(other bool) {
	if other {
		q.Selected = len(q.Choices)
	}
	q.Done, q.Skipped = false, false
}
func (q *Question) Complete(text string, sync, skip bool) bool {
	q.Skipped, q.Done, q.Answer = skip, true, nil
	if skip {
		return true
	}
	if q.Selected < len(q.Choices) {
		q.Answer = []string{q.Choices[q.Selected].Label}
		if (sync || q.Note) && text != "" {
			q.Answer = append(q.Answer, text)
		}
	} else if text != "" {
		q.Answer = []string{text}
	} else {
		q.Done = false
	}
	return q.Done
}
func (c *Call) Gaps() int {
	n := 0
	for _, q := range c.Questions {
		if q.Outcome == "" && !q.Sent && (!q.Done || c.Sync && q.Skipped) {
			n++
		}
	}
	return n
}
func (m *Questions) Advance() (step int, submit bool) {
	for j := m.Index + 1; j < len(m.Active.Questions); j++ {
		q := m.Active.Questions[j]
		if !q.Done && q.Outcome == "" && !q.Sent {
			return j - m.Index, false
		}
	}
	m.Confirm = m.Active.Gaps() > 0
	return 0, !m.Confirm
}

// View is detached plain data. It intentionally excludes draft text, secret
// answers, host payloads, and renderer-specific geometry.
type QuestionView struct {
	Selected                int
	Note, Pending           bool
	ID, Header, Text, State string
	Choices                 []Option
	Answer                  []string
	Secret                  bool
}
type CallView struct {
	Agent                  string `json:"agent,omitzero"`
	Sync, Local            bool
	ID, Thread, Turn, Item string
	Questions              []QuestionView
	Sent, Resolved         bool
}

// QuestionDockView excludes parked and active draft contents, including secrets.
type QuestionDockView struct {
	Count, Index, Gaps int
	Position, Total    int
	Confirm            bool
	Call               *CallView
}

// View positions the active question among the thread's calls, so the dock
// counts questions waiting in other calls too.
func (m *Questions) View(thread string) QuestionDockView {
	v := QuestionDockView{Index: m.Index, Confirm: m.Confirm}
	if m.Active != nil {
		call := m.Active.View()
		v.Call, v.Gaps = &call, m.Active.Gaps()
	}
	m.position(&v, threadPending(thread))
	return v
}

// position walks the kept calls: every question of the active call, so answered
// ones keep their place, and the pending questions of the others. Count is the
// number still pending.
func (m *Questions) position(v *QuestionDockView, keep func(*Call) bool) {
	v.Position, v.Total, v.Count = 0, 0, 0
	for _, call := range m.Calls {
		if !keep(call) {
			continue
		}
		for index, question := range call.Questions {
			if question.Outcome == "" && !question.Sent {
				v.Count++
			} else if call != m.Active {
				continue
			}
			if call == m.Active && index == m.Index {
				v.Position = v.Total
			}
			v.Total++
		}
	}
}

func (c *Call) View() CallView {
	v := CallView{Sync: c.Sync, Local: c.Local, ID: c.ID, Thread: c.Thread, Turn: c.Turn, Item: c.Item, Sent: c.Sent, Resolved: c.Resolved}
	for i, q := range c.Questions {
		state := q.Outcome
		if state == "" {
			switch {
			case c.Sent || q.Sent:
				state = "sending"
			case c.Sync || c.Local:
				state = "waiting"
			default:
				state = "open"
			}
		}
		r := QuestionView{Selected: q.Selected, Note: q.Note, Pending: q.Outcome == "" && !q.Sent, ID: c.questionID(i), Header: q.Header, Text: q.Question, State: state, Choices: slices.Clone(q.Choices), Secret: q.IsSecret}
		if !q.IsSecret && q.Outcome != "" {
			r.Answer = slices.Clone(q.Answer)
			if c.Sync && len(r.Answer) == 1 && q.Selected == len(q.Choices) {
				r.Answer[0] = "Other:\n\n" + r.Answer[0]
			}
		}
		v.Questions = append(v.Questions, r)
	}
	return v
}

// MoveChannel navigates pending calls without merging their answer authority.
func (m *Questions) MoveChannel(step int) {
	type position struct {
		call  *Call
		index int
	}
	var positions []position
	current := 0
	for _, call := range m.Calls {
		if call.Sent || call.Resolved {
			continue
		}
		for index, question := range call.Questions {
			if question.Outcome != "" || question.Sent {
				continue
			}
			if call == m.Active && index == m.Index {
				current = len(positions)
			}
			positions = append(positions, position{call, index})
		}
	}
	if len(positions) == 0 {
		return
	}
	next := positions[(current+step+len(positions))%len(positions)]
	m.Active, m.Index, m.Confirm = next.call, next.index, false
}

// ChannelView adds the shared question position while retaining the active call.
func (m *Questions) ChannelView(thread string) QuestionDockView {
	view := m.View(thread)
	m.position(&view, func(c *Call) bool { return !c.Sent && !c.Resolved })
	return view
}
