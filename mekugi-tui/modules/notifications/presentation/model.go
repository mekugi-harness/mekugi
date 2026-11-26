package presentation

import (
	"slices"

	"time"
)

// Model belongs to one attached view. Geometry and display receipts stay with
// the renderer; complete text does not depend on its size.
// Source: internal/router/composer_notice.go:14:29,156:200@b4d36aa2
type Model struct {
	Revision uint64
	Text     string
	Alert    bool
	Until    time.Time
	setAt    time.Time
}

func (m *Model) Set(text string, alert bool, now time.Time) {
	m.Revision++
	m.Text, m.Alert, m.Until = text, alert, time.Time{}
	m.setAt = now
	if text != "" && !alert {
		m.Until = now.Add(3 * time.Second)
	}
}

func (m *Model) Expire(now time.Time) bool {
	if m.Until.IsZero() || now.Before(m.Until) {
		return false
	}
	m.Set("", false, now)
	return true
}

// Progress belongs to one transcript, shared by every view attached to it. It
// keeps the row each progress notice occupies, so a retried claim from any
// view names the existing row instead of adding another.
type Progress struct{}

// DesktopPolicy consumes normalized settings. A nil event list allows every
// eligible event; an empty nonnil list allows none. The renderer owns readiness,
// focus observation, generic message text, output capability and signal encoding.
type DesktopPolicy struct {
	Enabled bool
	Always  bool
	Events  []string
}

func (p DesktopPolicy) Allows(event string, focused bool) bool {
	return p.Enabled && (!focused || p.Always) && (p.Events == nil || slices.Contains(p.Events, event))
}
