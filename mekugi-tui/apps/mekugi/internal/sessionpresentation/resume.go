package sessionpresentation

import (
	"time"
)

type ResumeRow struct {
	ID, Title, Workspace, Branch string
	Updated                      int64
}

// ResumeView publishes only admitted source facts and stable page targets.
type ResumeView struct {
	ID                                      uint64
	Workspace, Query, Problem               string
	Startup, All, Loading, HasMore, CanPage bool
	Rows                                    []ResumeRow
	Scanned                                 int
	PageAfter                               string
	LoadedAt                                time.Time
}
type ResumeActionKind uint8

const (
	ResumeNoAction ResumeActionKind = iota
	ResumeSearch
	ResumeToggleWorkspace
	ResumeChoose
	ResumeDismiss
	ResumeCancel
	ResumePage
)

type ResumeAction struct {
	Version              Version
	Picker               uint64
	Kind                 ResumeActionKind
	Query, Thread, After string
}

// ResumeChooser owns the attached chooser's source values and paging policy.
// Source: apps/mekugi/internal/router/app_server_resume_picker.go:24:54,201:213@0ce86dd5.
type ResumeChooser struct {
	ID        uint64
	Startup   bool   // No session exists yet; dismissal starts one and cancellation quits.
	Workspace string // Workspace filter; empty when unavailable.
	All       bool
	Query     string
	Rows      []ResumeRow
	Scanned   int

	Cursor   string
	Loading  bool
	Problem  string
	LoadedAt time.Time
}
