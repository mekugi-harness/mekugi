package terminalsession

import "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"

// StatusPanel owns navigation and geometry over one detached publication.
type StatusPanel struct {
	version         sessionpresentation.Version
	body            *sessionpresentation.PanelView
	Rect, Close     Rect // Body and close-control geometry, relative to Main.
	Top, Rows       int
	Features        bool
	Title, Controls string
	Groups          []string
	Fields          []sessionpresentation.StatusField
}

func (p *StatusPanel) Update(view sessionpresentation.View) {
	if view.Panel == nil {
		*p = StatusPanel{}
		return
	}
	if p.body == nil || p.version.Attachment != view.Version.Attachment || p.body.ID != view.Panel.ID {
		*p = StatusPanel{}
	}
	p.version, p.body = view.Version, view.Panel
	p.Title, p.Controls, p.Groups, p.Fields = "", "", nil, view.Panel.Fields
	if p.body.Session {
		p.renderMetrics()
	}
}
func (p *StatusPanel) Open() bool { return p.body != nil }

// Key consumes terminal input, returning only source-identified semantic effects.
// Source: apps/mekugi/internal/router/app_server_status.go:287:314 and
// app_server_session_metrics.go:48:95@630ce384.
func (p *StatusPanel) Key(key string) (action sessionpresentation.PanelAction, clearSelection bool) {
	if p.body == nil {
		return action, false
	}
	action.Version, action.Panel = p.version, p.body.ID
	if p.body.Session {
		switch key {
		case "\x1b[C", "\x1bOC", "\t", "\x1b[Z":
			p.Features = !p.Features
			p.Top = 0
			p.renderMetrics()
			if p.Features {
				action.Kind = sessionpresentation.PanelFeatures
			}
			return action, true
		case "r":
			action.Kind = sessionpresentation.PanelRefresh
			if p.Features {
				action.Kind = sessionpresentation.PanelFeatures
			}
			return action, false
		}
	}
	switch key {
	case "\x1b", keyLeft, keyLeftApplication, "q", "\r", "\x03":
		action.Kind = sessionpresentation.PanelClose
		clearSelection = true
	case "\x1b[A":
		p.Top = max(0, p.Top-1)
	case "\x1b[B":
		p.Top++
	case "\x1b[5~":
		p.Top = max(0, p.Top-max(1, p.Rows))
	case "\x1b[6~":
		p.Top += max(1, p.Rows)
	case "\x1b[H", "\x1b[1~":
		p.Top = 0
	}
	return action, clearSelection
}
