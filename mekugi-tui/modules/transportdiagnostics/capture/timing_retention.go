package capture

import (
	"context"
	"crypto/sha256"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"math"
	"os"

	responseevents "github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inspection"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
)

const (
	maxTimingRequests    = 64
	maxTimingRecordBytes = 512 << 10
)

// TimingRecord uses the existing shared budget and session ownership/expiry.
// The extra kind budget prevents optional diagnostics from filling that store.
var TimingRecord = recordkind.Spec{ID: "request-timing", Match: recordkind.HashedName("request-timing-", ".json"), MaxRecordBytes: maxTimingRecordBytes, Limit: 64 << 20}

type timingWindow struct {
	Version  int             `json:"version"`
	Thread   string          `json:"thread_id"`
	Evicted  uint64          `json:"evicted_requests"`
	Requests []RequestTiming `json:"requests"`
}

func timingName(thread string) string {
	return fmt.Sprintf("request-timing-%x.json", sha256.Sum256([]byte(thread)))
}

func readTimingWindow(store inspection.RetainedReader, thread string) (timingWindow, bool, error) {
	window := timingWindow{Version: 1, Thread: thread}
	data, err := store.ReadRetained(timingName(thread), maxTimingRecordBytes)
	if errors.Is(err, os.ErrNotExist) {
		return window, false, nil
	}
	if err != nil {
		return window, false, err
	}
	if err := json.Unmarshal(data, &window); err != nil {
		return window, false, err
	}
	if window.Version != 1 || window.Thread != thread || thread == "" || !responseevents.SafeDiagnosticIdentity(thread) || len(window.Requests) == 0 || len(window.Requests) > maxTimingRequests {
		return window, false, errors.New("invalid retained request timing window")
	}
	identities := make(map[string]bool, len(window.Requests))
	for _, request := range window.Requests {
		if request.CaptureID == "" || !responseevents.SafeDiagnosticIdentity(request.CaptureID) || identities[request.CaptureID] || request.Sequence == 0 || request.Started.IsZero() || request.Completed == nil || !validTimingOutcome(request.Outcome) || len(request.Attempts) > maxTimingAttempts {
			return window, false, errors.New("invalid retained request timing identity")
		}
		identities[request.CaptureID] = true
		phases := []*TimingPhase{request.ToolResultReady, request.ForwardStarted, request.Completed}
		var previous uint64
		for _, attempt := range request.Attempts {
			if attempt.Number <= previous || (attempt.Transport != "http" && attempt.Transport != "websocket") || (attempt.Status != 0 && (attempt.Status < 100 || attempt.Status > 599)) ||
				(attempt.Outcome != "unknown" && attempt.Outcome != "completed" && attempt.Outcome != "failed") || (attempt.Sent != nil) != (attempt.SendObservations == 1) {
				return window, false, errors.New("invalid retained request timing attempt")
			}
			previous = attempt.Number
			phases = append(phases, &attempt.Started, attempt.Sent, attempt.FirstOutput, attempt.Completed)
		}
		for _, phase := range phases {
			if phase != nil && (phase.At.IsZero() || phase.OffsetMillis < 0 || math.IsNaN(phase.OffsetMillis) || math.IsInf(phase.OffsetMillis, 0)) {
				return window, false, errors.New("invalid retained request timing phase")
			}
		}
	}
	return window, true, nil
}

func (*Service) InspectTiming(ctx context.Context, store inspection.RetainedReader, thread string) (jsontext.Value, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	result := struct {
		State           string `json:"state"`
		Coverage        string `json:"coverage"`
		FirstOutput     string `json:"first_output_definition"`
		ToolResultReady string `json:"tool_result_ready_state"`
		timingWindow
	}{State: "unavailable", Coverage: "recent completed observations only; crashes, unobserved requests and failed publications may be absent", FirstOutput: "first provider response body bytes or WebSocket message observed, not first generated token", ToolResultReady: "unavailable: no causal tool-result-to-request pairing"}
	var found bool
	var err error
	result.timingWindow, found, err = readTimingWindow(store, thread)
	if err != nil {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		// This automatically selected evidence is auxiliary to the AX report.
		// Discard partially decoded facts and raw errors, not unrelated AX data.
		return jsontext.Value(`{"state":"error","reason":"retained request timing could not be read or validated"}`), nil
	}
	if found {
		result.State = "observed_window"
	}
	return json.Marshal(&result)
}
