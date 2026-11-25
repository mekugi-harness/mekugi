package notice

import (
	"time"
)

// Source: internal/router/critical_errors.go:34:457@b4d36aa2 CriticalErrors

type Surface uint8

const (
	Error Surface = iota
	Progress
)

// Input is producer-owned text and identity, never execution authorization.
// Request notices wait for an identified turn's authoritative outcome.
type Input struct {
	ID, Session, Thread, Turn, Category, Reference, Text string
	Surface                                              Surface
	Request                                              bool
}

// Service holds bounded invocation recovery state. A nil service is disabled.
type Service struct {
}

type Message struct {
	ID, Text, Thread string
	Occurred         time.Time
}

// Batch keeps its captured repeat counts private; messages are detached values.
type Batch struct {
	Messages []Message
}

type Delivery struct{ Errors, Progress Batch }
