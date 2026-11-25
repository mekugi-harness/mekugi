package presentation

import (
	"time"
)

// AgentLifecycle records admitted observations, not host operation authority.
// Source: apps/mekugi/internal/router/app_server_events.go:244:311@[219eb509]
type AgentLifecycle struct {
	WorkTimer    WorkTimer `json:",omitzero"`
	Responding   bool      `json:",omitzero"`
	WaitingFor   string    `json:",omitzero"`
	Wait         *Value    `json:"-"`
	Retry        *Value    `json:"-"`
	Waiting      bool      `json:",omitzero"`
	Paused       bool      `json:",omitzero"`
	Final        bool      `json:",omitzero"`
	Started      time.Time `json:",omitzero"`
	LastResponse time.Time `json:",omitzero"`
	Turns        uint64    `json:",omitzero"`
}

// WorkTimer accumulates observed work intervals, never idle gaps.
// Source: apps/mekugi/internal/router/active_work_timer.go@[219eb509]
type WorkTimer struct {
	ElapsedNS int64     `json:"elapsed_ns,omitzero"`
	Since     time.Time `json:"since,omitzero"`
	Known     bool      `json:"known,omitzero"`
}
