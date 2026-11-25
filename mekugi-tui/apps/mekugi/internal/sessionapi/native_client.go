package sessionapi

import "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"

type NativeFailure struct {
	Status int
	Reason string
}

func (e *NativeFailure) Error() string { return e.Reason }

type NativeResult struct {
	Version sessionpresentation.Version `json:"version"`
}
