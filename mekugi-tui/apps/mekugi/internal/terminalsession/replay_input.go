package terminalsession

const (
	ReplayUp byte = 0x82 + iota
	ReplayDown
	ReplayRight
	ReplayLeft
	ReplayPageUp
	ReplayPageDown
	ReplayHome
	ReplayEnd
)

// ReplayInput decodes terminal reports before interpreting playback controls. State persists
// across reads, so fragmented mouse or navigation sequences cannot quit replay.
// Source: apps/mekugi/internal/router/session_ui_replay_input.go:25:101@70e7ce50 uiReplayInput
type ReplayInput struct {
}
