package sessionapi

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
)

// RemoteMutation is the public semantic input shape. Answers use the public
// question contract; native preparation and final-batch fields stay private.
type RemoteMutation struct {
	Attachment string                      `json:"attachment"`
	Version    sessionpresentation.Version `json:"version"`
	Operation  RemoteOperation             `json:"operation"`
}

type RemoteOperation struct {
	NativeOperation
	Answers *Answer `json:"answers,omitempty"`
}

// remoteMutation converts the public wire value at the authority boundary.
// The pending prompt owner resolves choices; transport cannot infer state.
