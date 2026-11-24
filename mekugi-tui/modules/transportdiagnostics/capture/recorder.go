// Package capture prepares private native diagnostics and reads retained captures.
package capture

import (
	"crypto/rand"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"io"
	"os"
	"sync"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/persistence"
)

const schemaVersion = 7

type ProviderUsage struct {
	EvidenceComplete bool                       `json:"evidence_complete"`
	InputTokens      uint64                     `json:"input_tokens"`
	CachedTokens     uint64                     `json:"cached_input_tokens"`
	OutputTokens     uint64                     `json:"output_tokens"`
	ReasoningTokens  uint64                     `json:"reasoning_tokens"`
	OutputThroughput inference.OutputThroughput `json:"output_throughput,omitzero"`
}

type payloadMetrics struct {
	Bytes uint64 `json:"bytes"`
}

// captureRecord is the usage projection of retained v7 provider records.
type captureRecord struct {
	SchemaVersion    int            `json:"schema_version"`
	Boundary         string         `json:"boundary"`
	CaptureID        string         `json:"capture_id"`
	ProviderAttempt  uint64         `json:"provider_attempt,omitzero"`
	TurnID           string         `json:"turn_id,omitempty"`
	ThreadID         string         `json:"thread_id"`
	CapturedAt       time.Time      `json:"captured_at"`
	RequestModel     string         `json:"request_model"`
	DurationMillis   int64          `json:"duration_ms,omitempty"`
	ResponseComplete bool           `json:"response_complete"`
	ResponseStatus   string         `json:"response_status"`
	CaptureError     string         `json:"capture_error"`
	Usage            *ProviderUsage `json:"usage,omitempty"`
}

type Config struct {
	Output string
	Writes *persistence.Counter
}

const maxExchanges = 64

type Recorder struct {
	mu       sync.Mutex
	writes   *persistence.Counter
	output   *os.File
	snapshot MetricsSnapshot
}

func (*Service) Open(config Config) (*Recorder, error) {
	r := &Recorder{writes: config.Writes, snapshot: MetricsSnapshot{Schema: "mekugi.native.responses.v1", Mode: "native"}}
	if config.Output != "" {
		file, err := os.OpenFile(config.Output, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
		if err != nil {
			return nil, err
		}
		r.output = file
	}
	return r, nil
}

func (r *Recorder) Close() error {
	if r.output != nil {
		return r.output.Close()
	}
	return nil
}

// ObserveResponse keeps one content-free record per native completed response.
// Counts and timings are evidence only: no inferred retries or requests.
func (r *Recorder) ObserveResponse(thread, turn, model string, counts inference.Usage, duration time.Duration) {
	var usage *ProviderUsage
	if counts.TotalsKnown && !counts.Inconsistent {
		usage = &ProviderUsage{EvidenceComplete: !counts.Incomplete,
			InputTokens: counts.InputTokens, CachedTokens: counts.InputTokens - counts.UncachedInputTokens,
			OutputTokens: counts.OutputTokens, ReasoningTokens: counts.ReasoningTokens}
	}
	r.observe(captureRecord{SchemaVersion: schemaVersion, Boundary: "native_response",
		ThreadID: thread, TurnID: turn, RequestModel: model, CapturedAt: time.Now(), ResponseComplete: true,
		ResponseStatus: "completed", DurationMillis: duration.Milliseconds(), Usage: usage})
}

// ObserveFailure records the native terminal error notification without copying
// upstream error bodies or URLs.
func (r *Recorder) ObserveFailure(thread, turn string) {
	r.observe(captureRecord{SchemaVersion: schemaVersion, Boundary: "native_turn",
		ThreadID: thread, TurnID: turn, CapturedAt: time.Now(), ResponseStatus: "failed"})
}

func (r *Recorder) observe(record captureRecord) {
	r.mu.Lock()
	defer r.mu.Unlock()
	s := &r.snapshot
	e := exchangeMetrics{Sequence: s.Capture.Records + 1, ThreadID: record.ThreadID,
		Model: record.RequestModel, ResponseComplete: record.ResponseComplete, Status: record.ResponseStatus}
	if record.DurationMillis > 0 {
		e.DurationMillis = uint64(record.DurationMillis)
	}
	if record.ResponseComplete {
		s.Requests.Completed++
		switch u := record.Usage; {
		case u == nil:
			s.Usage.MissingAttempts++
		default:
			e.Usage = &usageMetrics{InputTokens: u.InputTokens, CachedInputTokens: u.CachedTokens,
				UncachedInputTokens: u.InputTokens - u.CachedTokens, OutputTokens: u.OutputTokens, ReasoningTokens: u.ReasoningTokens}
			if e.DurationMillis > 0 {
				e.Usage.OutputThroughput = inference.OutputThroughput{
					OutputTokens: u.OutputTokens, DurationNanos: e.DurationMillis * uint64(time.Millisecond), MeasuredRequests: 1,
				}
				s.Usage.OutputThroughput.Add(e.Usage.OutputThroughput)
			}
			if u.EvidenceComplete {
				e.Usage.CompleteAttempts, e.Usage.CachedObserved, e.Usage.ReasoningObserved = 1, true, true
			} else {
				e.Usage.IncompleteAttempts = 1
			}
			s.Usage.CompleteAttempts += e.Usage.CompleteAttempts
			s.Usage.IncompleteAttempts += e.Usage.IncompleteAttempts
			s.Usage.CachedObserved = s.Usage.CachedObserved || u.EvidenceComplete
			s.Usage.ReasoningObserved = s.Usage.ReasoningObserved || u.EvidenceComplete
			s.Usage.InputTokens += u.InputTokens
			s.Usage.OutputTokens += u.OutputTokens
			if u.EvidenceComplete {
				s.Usage.CachedInputTokens += u.CachedTokens
				s.Usage.UncachedInputTokens += u.InputTokens - u.CachedTokens
				s.Usage.ReasoningTokens += u.ReasoningTokens
			}
		}
	} else {
		s.Requests.Failed++
	}
	s.Capture.Records++
	if len(s.Exchanges) == maxExchanges {
		copy(s.Exchanges, s.Exchanges[1:])
		s.Exchanges = s.Exchanges[:maxExchanges-1]
		s.Capture.DroppedExchangeDetails++
	}
	s.Exchanges = append(s.Exchanges, e)
	if r.output != nil {
		record.CaptureID = rand.Text()
		if err := json.MarshalEncode(jsontext.NewEncoder(r.writes.Writer(r.output)), &record); err != nil {
			s.Capture.WriteErrors++
		}
	}
}

func (r *Recorder) Snapshot() MetricsSnapshot {
	r.mu.Lock()
	defer r.mu.Unlock()
	s := r.snapshot.Clone()
	s.StorageWrites = r.writes.Snapshot()
	return s
}

func (r *Recorder) WriteMetrics(writer io.Writer) error {
	s := r.Snapshot()
	return json.MarshalEncode(jsontext.NewEncoder(writer), &s)
}
