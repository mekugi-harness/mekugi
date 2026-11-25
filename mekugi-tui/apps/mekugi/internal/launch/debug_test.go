package launch

import (
	"bytes"
	json "encoding/json/v2"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
)

func TestOpenDebugOutputLinksCustomCapture(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	customCapture := filepath.Join(t.TempDir(), "custom.jsonl")

	session, err := openDebugOutput(true, customCapture, "")
	if err != nil {
		t.Fatalf("openDebugOutput failed: %v", err)
	}
	defer func() { _ = session.Close() }()

	if session.CapturePath() != customCapture {
		t.Fatalf("CapturePath() = %q, want %q", session.CapturePath(), customCapture)
	}
	target, err := os.Readlink(filepath.Join(session.dir, "capture.jsonl"))
	if err != nil {
		t.Fatalf("readlink capture.jsonl: %v", err)
	}
	if target != customCapture {
		t.Fatalf("bundle capture symlink target = %q, want %q", target, customCapture)
	}
}

func TestHeadlessCaptureV7RecordsAndMetrics(t *testing.T) {
	dir := t.TempDir()
	capturePath := filepath.Join(dir, "capture.jsonl")
	metricsPath := filepath.Join(dir, "metrics.json")

	capture, err := openHeadlessCapture(capturePath)
	if err != nil {
		t.Fatalf("openHeadlessCapture failed: %v", err)
	}
	capture.ObserveResponse("thread-1", "turn-1", "gpt-5", inference.Usage{
		TotalsKnown:         true,
		InputTokens:         100,
		UncachedInputTokens: 60,
		OutputTokens:        25,
		ReasoningTokens:     10,
	}, 500*time.Millisecond)
	capture.ObserveResponse("thread-1", "turn-2", "gpt-5", inference.Usage{
		TotalsKnown:         true,
		InputTokens:         50,
		UncachedInputTokens: 50,
		OutputTokens:        15,
		Incomplete:          true,
	}, 250*time.Millisecond)
	capture.ObserveResponse("thread-1", "turn-3", "gpt-5", inference.Usage{
		TotalsKnown:  true,
		InputTokens:  999,
		OutputTokens: 999,
		Inconsistent: true,
	}, 100*time.Millisecond)
	capture.ObserveResponse("thread-1", "turn-4", "gpt-5", inference.Usage{}, 0)
	capture.ObserveFailure("thread-1", "turn-5")

	if err := capture.WriteMetrics(metricsPath); err != nil {
		t.Fatalf("WriteMetrics failed: %v", err)
	}
	if err := capture.Close(); err != nil {
		t.Fatalf("Close failed: %v", err)
	}

	rawCapture, err := os.ReadFile(capturePath)
	if err != nil {
		t.Fatalf("read capture: %v", err)
	}
	var records []captureRecord
	for line := range bytes.SplitSeq(bytes.TrimSpace(rawCapture), []byte("\n")) {
		var rec captureRecord
		if err := json.Unmarshal(line, &rec); err != nil {
			t.Fatalf("unmarshal capture record: %v", err)
		}
		records = append(records, rec)
	}
	if len(records) != 5 {
		t.Fatalf("record count = %d, want 5", len(records))
	}
	if records[0].SchemaVersion != 7 || records[0].Boundary != "native_response" || records[0].ResponseStatus != "completed" || !records[0].ResponseComplete || records[0].CaptureID == "" {
		t.Fatalf("unexpected completed record: %+v", records[0])
	}
	if records[0].Usage == nil || !records[0].Usage.EvidenceComplete || records[0].Usage.CachedTokens != 40 {
		t.Fatalf("unexpected completed usage: %+v", records[0].Usage)
	}
	if records[2].Usage != nil || records[3].Usage != nil {
		t.Fatalf("inconsistent/missing samples should emit nil usage: %+v / %+v", records[2].Usage, records[3].Usage)
	}
	if records[4].SchemaVersion != 7 || records[4].Boundary != "native_turn" || records[4].ResponseStatus != "failed" || records[4].ResponseComplete {
		t.Fatalf("unexpected failure record: %+v", records[4])
	}

	rawMetrics, err := os.ReadFile(metricsPath)
	if err != nil {
		t.Fatalf("read metrics: %v", err)
	}
	var metrics struct {
		Schema   string                `json:"schema"`
		Mode     string                `json:"mode"`
		Requests headlessRequestTotals `json:"requests"`
		Usage    headlessUsageMetrics  `json:"usage"`
	}
	if err := json.Unmarshal(rawMetrics, &metrics); err != nil {
		t.Fatalf("unmarshal metrics: %v", err)
	}
	if metrics.Requests.Completed != 4 || metrics.Requests.Failed != 1 {
		t.Fatalf("unexpected request totals: %+v", metrics.Requests)
	}
	if metrics.Usage.CompleteAttempts != 1 || metrics.Usage.IncompleteAttempts != 1 || metrics.Usage.MissingAttempts != 2 {
		t.Fatalf("unexpected attempt coverage: %+v", metrics.Usage)
	}
	if metrics.Usage.InputTokens != 150 || metrics.Usage.OutputTokens != 40 ||
		metrics.Usage.CachedInputTokens != 40 || metrics.Usage.UncachedInputTokens != 60 ||
		metrics.Usage.ReasoningTokens != 10 || !metrics.Usage.CachedObserved || !metrics.Usage.ReasoningObserved {
		t.Fatalf("unexpected token totals: %+v", metrics.Usage)
	}
	if metrics.Usage.OutputThroughput.OutputTokens != 40 || metrics.Usage.OutputThroughput.MeasuredRequests != 2 {
		t.Fatalf("unexpected throughput: %+v", metrics.Usage)
	}
}
