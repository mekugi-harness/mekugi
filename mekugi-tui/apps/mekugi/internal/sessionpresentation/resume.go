package sessionpresentation

import (
	"time"
)

type ResumeRow struct {
	ID, Title, Preview, Workspace, Branch string
	Updated                               int64
}

// ResumeView publishes only admitted source facts and stable page targets.
type ResumeView struct {
	ID                     uint64
	Workspace, Problem     string
	Startup, All, Archived bool
	Rows                   []ResumeRow
	PageAfter              string
	LoadedAt               time.Time
}
type ResumeActionKind uint8

const (
	ResumeNoAction ResumeActionKind = iota
	ResumeToggleWorkspace
	ResumeToggleArchived
	ResumeChoose
	ResumeDismiss
	ResumeCancel
	ResumePage
)

type ResumeAction struct {
	Version       Version
	Picker        uint64
	Kind          ResumeActionKind
	Thread, After string
}
