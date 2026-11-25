package launch

import (
	"crypto/rand"
	json "encoding/json/v2"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
)

const axReadOutputEnvironment = "MEKUGI_AX_OUTPUT"

type debugSession struct {
	dir         string
	capturePath string
	metricsPath string
	readLogPath string
	mu          sync.Mutex
	eventsFile  *os.File
}

func openDebugOutput(enabled bool, capturePath, readLogPath string) (*debugSession, error) {
	if !enabled {
		return nil, nil
	}
	root, err := debugStorageDirectory()
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(root, 0o700); err != nil {
		return nil, fmt.Errorf("create debug directory: %w", err)
	}
	dir, err := os.MkdirTemp(root, time.Now().UTC().Format("20060102T150405Z-"))
	if err != nil {
		return nil, fmt.Errorf("create debug session directory: %w", err)
	}
	if capturePath == "" {
		capturePath = filepath.Join(dir, "capture.jsonl")
	}
	capturePath, err = filepath.Abs(capturePath)
	if err != nil {
		return nil, fmt.Errorf("resolve debug capture path: %w", err)
	}
	// Keep custom capture output reachable from the standard bundle path so
	// archiving the debug directory retains the capture.
	if bundleCapture := filepath.Join(dir, "capture.jsonl"); capturePath != bundleCapture {
		if err := os.Symlink(capturePath, bundleCapture); err != nil {
			return nil, fmt.Errorf("link debug capture: %w", err)
		}
	}
	if readLogPath == "" {
		readLogPath = filepath.Join(dir, "ax-reads.jsonl")
	}
	eventsFile, err := os.OpenFile(filepath.Join(dir, "events.jsonl"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return nil, fmt.Errorf("open debug events log: %w", err)
	}
	return &debugSession{
		dir:         dir,
		capturePath: capturePath,
		metricsPath: filepath.Join(dir, "metrics.json"),
		readLogPath: readLogPath,
		eventsFile:  eventsFile,
	}, nil
}

func (d *debugSession) CapturePath() string { return d.capturePath }
func (d *debugSession) MetricsPath() string { return d.metricsPath }
func (d *debugSession) ReadLogPath() string { return d.readLogPath }

func (d *debugSession) Event(fields map[string]any) {
	if d == nil || d.eventsFile == nil {
		return
	}
	data, err := json.Marshal(fields)
	if err != nil {
		return
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	_, _ = d.eventsFile.Write(append(data, '\n'))
}

func (d *debugSession) Close() error {
	if d == nil || d.eventsFile == nil {
		return nil
	}
	d.mu.Lock()
	f := d.eventsFile
	d.eventsFile = nil
	d.mu.Unlock()
	return f.Close()
}

const captureSchemaVersion = 7

type captureProviderUsage struct {
	EvidenceComplete bool   `json:"evidence_complete"`
	InputTokens      uint64 `json:"input_tokens"`
	CachedTokens     uint64 `json:"cached_input_tokens"`
	OutputTokens     uint64 `json:"output_tokens"`
	ReasoningTokens  uint64 `json:"reasoning_tokens"`
}

// captureRecord matches the published v7 provider capture JSONL shape.
type captureRecord struct {
	SchemaVersion    int                   `json:"schema_version"`
	Boundary         string                `json:"boundary"`
	CaptureID        string                `json:"capture_id"`
	TurnID           string                `json:"turn_id,omitempty"`
	ThreadID         string                `json:"thread_id"`
	CapturedAt       time.Time             `json:"captured_at"`
	RequestModel     string                `json:"request_model"`
	DurationMillis   int64                 `json:"duration_ms,omitempty"`
	ResponseComplete bool                  `json:"response_complete"`
	ResponseStatus   string                `json:"response_status"`
	CaptureError     string                `json:"capture_error"`
	Usage            *captureProviderUsage `json:"usage,omitempty"`
}

type headlessUsageMetrics struct {
	CachedObserved      bool                       `json:"cached_observed,omitzero"`
	ReasoningObserved   bool                       `json:"reasoning_observed,omitzero"`
	OutputThroughput    inference.OutputThroughput `json:"output_throughput,omitzero"`
	CompleteAttempts    uint64                     `json:"complete_attempts"`
	IncompleteAttempts  uint64                     `json:"incomplete_attempts"`
	MissingAttempts     uint64                     `json:"missing_attempts"`
	InputTokens         uint64                     `json:"input_tokens"`
	CachedInputTokens   uint64                     `json:"cached_input_tokens"`
	UncachedInputTokens uint64                     `json:"uncached_input_tokens"`
	OutputTokens        uint64                     `json:"output_tokens"`
	ReasoningTokens     uint64                     `json:"reasoning_tokens"`
}

type headlessRequestTotals struct {
	Completed uint64 `json:"completed"`
	Failed    uint64 `json:"failed"`
}

type headlessCapture struct {
	mu       sync.Mutex
	file     *os.File
	requests headlessRequestTotals
	usage    headlessUsageMetrics
}

func openHeadlessCapture(path string) (*headlessCapture, error) {
	if path == "" {
		return &headlessCapture{}, nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return nil, err
	}
	return &headlessCapture{file: f}, nil
}

func (c *headlessCapture) ObserveResponse(threadID, turnID, model string, counts inference.Usage, duration time.Duration) {
	if c == nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.requests.Completed++
	var usage *captureProviderUsage
	if counts.TotalsKnown && !counts.Inconsistent {
		cached := counts.InputTokens - counts.UncachedInputTokens
		usage = &captureProviderUsage{
			EvidenceComplete: !counts.Incomplete,
			InputTokens:      counts.InputTokens,
			CachedTokens:     cached,
			OutputTokens:     counts.OutputTokens,
			ReasoningTokens:  counts.ReasoningTokens,
		}
		c.usage.InputTokens += usage.InputTokens
		c.usage.OutputTokens += usage.OutputTokens
		if durationMS := duration.Milliseconds(); durationMS > 0 {
			c.usage.OutputThroughput.Add(inference.OutputThroughput{
				OutputTokens:     usage.OutputTokens,
				DurationNanos:    uint64(durationMS) * uint64(time.Millisecond),
				MeasuredRequests: 1,
			})
		}
		if usage.EvidenceComplete {
			c.usage.CompleteAttempts++
			c.usage.CachedObserved = true
			c.usage.ReasoningObserved = true
			c.usage.CachedInputTokens += usage.CachedTokens
			c.usage.UncachedInputTokens += usage.InputTokens - usage.CachedTokens
			c.usage.ReasoningTokens += usage.ReasoningTokens
		} else {
			c.usage.IncompleteAttempts++
		}
	} else {
		c.usage.MissingAttempts++
	}
	if c.file != nil {
		record := captureRecord{
			SchemaVersion:    captureSchemaVersion,
			Boundary:         "native_response",
			CaptureID:        rand.Text(),
			ThreadID:         threadID,
			TurnID:           turnID,
			RequestModel:     model,
			CapturedAt:       time.Now(),
			ResponseComplete: true,
			ResponseStatus:   "completed",
			DurationMillis:   duration.Milliseconds(),
			Usage:            usage,
		}
		if data, err := json.Marshal(&record); err == nil {
			_, _ = c.file.Write(append(data, '\n'))
		}
	}
}

func (c *headlessCapture) ObserveFailure(threadID, turnID string) {
	if c == nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.requests.Failed++
	if c.file != nil {
		record := captureRecord{
			SchemaVersion:  captureSchemaVersion,
			Boundary:       "native_turn",
			CaptureID:      rand.Text(),
			ThreadID:       threadID,
			TurnID:         turnID,
			CapturedAt:     time.Now(),
			ResponseStatus: "failed",
		}
		if data, err := json.Marshal(&record); err == nil {
			_, _ = c.file.Write(append(data, '\n'))
		}
	}
}

func (c *headlessCapture) WriteMetrics(path string) error {
	if c == nil || path == "" {
		return nil
	}
	c.mu.Lock()
	payload := struct {
		Schema   string                `json:"schema"`
		Mode     string                `json:"mode"`
		Requests headlessRequestTotals `json:"requests"`
		Usage    headlessUsageMetrics  `json:"usage"`
	}{
		Schema:   "mekugi.native.responses.v1",
		Mode:     "native",
		Requests: c.requests,
		Usage:    c.usage,
	}
	c.mu.Unlock()
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	return os.WriteFile(path, append(data, '\n'), 0o600)
}

func (c *headlessCapture) Close() error {
	if c == nil || c.file == nil {
		return nil
	}
	c.mu.Lock()
	f := c.file
	c.file = nil
	c.mu.Unlock()
	return f.Close()
}
