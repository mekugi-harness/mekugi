package capture

import "time"

const maxTimingAttempts = 8

type TimingPhase struct {
	At           time.Time `json:"at"`
	OffsetMillis float64   `json:"offset_ms"`
}

type AttemptTiming struct {
	Number           uint64       `json:"number"`
	Transport        string       `json:"transport"`
	Started          TimingPhase  `json:"started"`
	Sent             *TimingPhase `json:"request_sent"`
	SendObservations uint64       `json:"send_observations"`
	FirstOutput      *TimingPhase `json:"first_output"`
	Completed        *TimingPhase `json:"completed"`
	Status           int          `json:"status"`
	Outcome          string       `json:"outcome"`
}

type RequestTiming struct {
	CaptureID       string          `json:"capture_id"`
	Sequence        uint64          `json:"request_sequence"`
	Started         time.Time       `json:"started_at"`
	ToolResultReady *TimingPhase    `json:"tool_result_ready"`
	ForwardStarted  *TimingPhase    `json:"forward_started"`
	Completed       *TimingPhase    `json:"completed"`
	Outcome         string          `json:"outcome"`
	Attempts        []AttemptTiming `json:"attempts"`
	OmittedAttempts uint64          `json:"omitted_attempts"`
}

func validTimingOutcome(outcome string) bool {
	switch outcome {
	case "unknown", "completed", "failed", "canceled_before_response", "canceled_after_response", "timed_out", "stream_idle_timed_out":
		return true
	default:
		return false
	}
}
