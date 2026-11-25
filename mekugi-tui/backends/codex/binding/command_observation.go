package binding

// Source: apps/mekugi/internal/router/exec_track.go:143:157 and
// mekugi-tui/apps/mekugi/internal/router/app_server_events.go:755:791@b27b4c66.

// CommandObservation preserves command identity and supplied output/outcome.
// Decoding confers neither execution nor report-correlation authority.
type CommandObservation struct {
	Key            [3]string // Thread, turn, item.
	Command, Delta string
	Output         *string
	Exit           *int
}
