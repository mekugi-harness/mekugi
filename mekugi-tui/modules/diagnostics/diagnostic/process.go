package diagnostic

import (
	"time"
)

// Each process retains the latest two stderr segments, at most 1 MiB total.

// ProcessEvent records process facts independently of session/request diagnostics.
type ProcessEvent struct {
	Event          string    `json:"event"`
	Time           time.Time `json:"time"`
	PID            int       `json:"pid"`
	Socket         string    `json:"socket"`
	Executable     string    `json:"executable"`
	Build          string    `json:"build"`
	Revision       string    `json:"revision,omitempty"`
	Modified       bool      `json:"modified,omitzero"`
	GoVersion      string    `json:"go_version"`
	ExitCode       *int      `json:"exit_code,omitempty"`
	Signal         string    `json:"signal,omitempty"`
	Error          string    `json:"error,omitempty"`
	StderrError    string    `json:"stderr_error,omitempty"`
	RetentionError string    `json:"retention_error,omitempty"`
	StderrBytes    int64     `json:"stderr_bytes"`
}

// ProcessLog belongs to the detached supervisor, never to an interactive client.
// Its sole writer drains stderr even after a storage error, preserving execution.
type ProcessLog struct {
}
