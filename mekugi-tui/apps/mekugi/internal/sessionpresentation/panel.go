package sessionpresentation

// PanelView contains source values, never layout or a request handle.
type PanelView struct {
	ID            uint64
	Session       bool
	Fields        []StatusField
	FeatureFields []StatusField
	Workspace     string
}

// StatusField preserves numeric observations independently of formatted labels.
type StatusField struct {
	Group, Label, Value, Detail string
	Remaining                   *float64
	Alert, Path                 bool
	// Values replaces Value with one cell per column of a comparison group.
	// The group's Heading field names those columns.
	Values  []string
	Heading bool
}

type PanelActionKind uint8

const (
	PanelNoAction PanelActionKind = iota
	PanelClose
	PanelRefresh
	PanelFeatures
)

type PanelAction struct {
	Version Version
	Panel   uint64
	Kind    PanelActionKind
}

// PanelState retains structured presentation, independently of account requests
// and refresh timing. Publication detaches these values with the session view.
// Source: apps/mekugi/internal/router/app_server_status.go:25:45,124:127@0ce86dd5.
type PanelState struct {
	PanelView
	Body, Tokens, Account []StatusField
}
