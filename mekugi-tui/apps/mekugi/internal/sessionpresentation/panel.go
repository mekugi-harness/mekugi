package sessionpresentation

import (
	transportcapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/transportdiagnostics/capture"
)

// PanelView contains source values, never layout or a request handle.
type PanelView struct {
	// MetricViews names the metric tabs in order for renderers; empty outside /session.
	MetricViews               []string
	MetricTab, MetricIndex    int
	MetricExchange            uint64
	ID                        uint64
	Fields                    []StatusField
	Metrics                   *transportcapture.MetricsSnapshot
	Features                  *FeatureMetrics
	Workspace, DebugDirectory string
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
	PanelMetrics
)

type PanelAction struct {
	Version  Version
	Panel    uint64
	Kind     PanelActionKind
	Tab      int    `json:",omitzero"`
	Exchange uint64 `json:",omitzero"`
}

// PanelState retains structured presentation, independently of account requests
// and refresh timing. Publication detaches these values with the session view.
// Source: apps/mekugi/internal/router/app_server_status.go:25:45,124:127@0ce86dd5.
type PanelState struct {
	PanelView
	Body, Tokens, Account []StatusField
}
