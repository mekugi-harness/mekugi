package terminalsession

import (
	"strings"
	"unicode/utf8"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
)

const resumePickerNear = 5

// ResumePicker owns local search, selection and geometry, never host reads.
type ResumePicker struct {
	version                           sessionpresentation.Version
	body                              *sessionpresentation.ResumeView
	main, query                       string
	rows                              []sessionpresentation.ResumeRow
	Selected, Top, Viewport, RowStart int
}

func (p *ResumePicker) Update(v sessionpresentation.View) {
	if v.Resume == nil {
		*p = ResumePicker{}
		return
	}
	if p.body == nil || p.version.Attachment != v.Version.Attachment || p.body.ID != v.Resume.ID {
		*p = ResumePicker{}
	} else if p.body.All != v.Resume.All || p.body.Archived != v.Resume.Archived {
		p.Selected, p.Top = 0, 0
	}
	p.version, p.main = v.Version, v.Main.ID
	if p.body != v.Resume {
		p.body = v.Resume
		p.filterRows()
	}
	p.Selected = max(0, min(p.Selected, len(p.rows)-1))
}

func (p *ResumePicker) filterRows() {
	if p.query == "" {
		p.rows = p.body.Rows
		return
	}
	p.rows = nil
	query := strings.ToLower(p.query)
	for _, row := range p.body.Rows {
		for _, field := range [...]string{row.Title, row.Preview, row.ID, row.Branch, row.Workspace} {
			if strings.Contains(strings.ToLower(field), query) {
				p.rows = append(p.rows, row)
				break
			}
		}
	}
}

func (p *ResumePicker) setQuery(query string) {
	if p.query == query {
		return
	}
	p.query = query
	var selected string
	if query == "" && len(p.rows) != 0 {
		selected = p.rows[p.Selected].ID
	}
	p.Selected, p.Top = 0, 0
	p.filterRows()
	if selected != "" {
		for i, row := range p.rows {
			if row.ID == selected {
				p.Selected = i
				break
			}
		}
	}
}

func (p *ResumePicker) Open() bool { return p.body != nil }
func (p *ResumePicker) action(kind sessionpresentation.ResumeActionKind) sessionpresentation.ResumeAction {
	return sessionpresentation.ResumeAction{Version: p.version, Picker: p.body.ID, Kind: kind}
}
func (p *ResumePicker) PageDemand() sessionpresentation.ResumeAction {
	if p.body == nil || p.body.PageAfter == "" || len(p.rows) > max(p.Selected+resumePickerNear, p.Viewport) {
		return sessionpresentation.ResumeAction{}
	}
	action := p.action(sessionpresentation.ResumePage)
	action.After = p.body.PageAfter
	return action
}

// Source: apps/mekugi/internal/router/app_server_resume_picker.go:234:300@630ce384.
func (p *ResumePicker) Key(key string) sessionpresentation.ResumeAction {
	if p.body == nil {
		return sessionpresentation.ResumeAction{}
	}
	v := p.body
	move := func(to int) { p.Selected = max(0, min(len(p.rows)-1, to)) }
	action := p.action(sessionpresentation.ResumeNoAction)
	switch key {
	case "\x1b":
		if p.query != "" {
			p.setQuery("")
		} else {
			action.Kind = sessionpresentation.ResumeDismiss
		}
	case "\x03":
		action.Kind = sessionpresentation.ResumeCancel
	case "\x1b[A", "\x1bOA", "\x10":
		move(p.Selected - 1)
	case "\x1b[B", "\x1bOB", "\x0e":
		move(p.Selected + 1)
	case "\x1b[5~":
		move(p.Selected - max(1, p.Viewport))
	case "\x1b[6~":
		move(p.Selected + max(1, p.Viewport))
	case "\x1b[H", "\x1bOH":
		move(0)
	case "\x1b[F", "\x1bOF":
		move(len(p.rows) - 1)
	case "\t":
		if v.Workspace != "" {
			action.Kind = sessionpresentation.ResumeToggleWorkspace
		}
	case "\x1b[D", "\x1bOD", "\x1b[C", "\x1bOC":
		action.Kind = sessionpresentation.ResumeToggleArchived
	case "\r":
		if len(p.rows) > 0 {
			action.Kind, action.Thread = sessionpresentation.ResumeChoose, p.rows[p.Selected].ID
		}
	case "\x7f", "\b":
		if p.query != "" {
			_, n := utf8.DecodeLastRuneInString(p.query)
			p.setQuery(p.query[:len(p.query)-n])
		}
	default:
		if key == "" || key[0] < 32 || key[0] == 0x7f {
			return action
		}
		p.setQuery(p.query + key)
	}
	if action.Kind == sessionpresentation.ResumeNoAction {
		return p.PageDemand()
	}
	return action
}
func (p *ResumePicker) Mouse(button, y int) sessionpresentation.ResumeAction {
	if p.body == nil {
		return sessionpresentation.ResumeAction{}
	}
	switch button {
	case 64:
		return p.Key("\x1b[A")
	case 65:
		return p.Key("\x1b[B")
	case 0:
		if row := p.Top + y - p.RowStart; y >= p.RowStart && y-p.RowStart < p.Viewport && row < len(p.rows) {
			p.Selected = row
		}
	}
	return sessionpresentation.ResumeAction{}
}
