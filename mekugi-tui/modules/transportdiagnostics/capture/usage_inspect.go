// Source: capturer/usage_inspect.go@b5162407.
package capture

import (
	"bufio"
	"context"
	jsonv1 "encoding/json"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"syscall"
	"time"
	"unicode/utf8"
)

// UsageInspection keeps provider-consumption evidence separate from local byte
// measurements. Missing/incomplete records never count as zero-token successes.
type UsageInspection struct {
	State           string         `json:"state"`
	ObservedRecords uint64         `json:"observed_records"`
	MissingRecords  uint64         `json:"missing_records"`
	Tokens          *ProviderUsage `json:"tokens,omitempty"`
}

type UsageInspectionFilter struct {
	Since, Until        time.Time
	Model, ExcludeModel string
	Threads             map[string]bool
}

func (f UsageInspectionFilter) accepts(thread, model string, at time.Time) bool {
	match, _ := filepath.Match(f.Model, model)
	exclude, _ := filepath.Match(f.ExcludeModel, model)
	return f.Threads[thread] && !at.Before(f.Since) && at.Before(f.Until) && match && !exclude
}

func (entry *UsageInspection) add(valid bool, usage *ProviderUsage) error {
	if !valid {
		entry.MissingRecords++
	} else {
		if usage.CachedTokens > usage.InputTokens || usage.ReasoningTokens > usage.OutputTokens {
			return errors.New("invalid provider usage totals")
		}
		if entry.Tokens == nil {
			entry.Tokens = &ProviderUsage{EvidenceComplete: true}
		}
		for _, pair := range [][2]*uint64{
			{&entry.Tokens.InputTokens, &usage.InputTokens},
			{&entry.Tokens.CachedTokens, &usage.CachedTokens},
			{&entry.Tokens.OutputTokens, &usage.OutputTokens},
			{&entry.Tokens.ReasoningTokens, &usage.ReasoningTokens},
		} {
			if math.MaxUint64-*pair[0] < *pair[1] {
				return errors.New("provider usage overflow")
			}
			*pair[0] += *pair[1]
		}
		entry.ObservedRecords++
	}
	entry.State = "observed"
	if entry.MissingRecords > 0 {
		entry.State = "incomplete"
	}
	return nil
}

// InspectProviderUsage reads sanitized capture JSONL, never executable history.
func (*Service) InspectProviderUsage(ctx context.Context, path string, filter UsageInspectionFilter) (map[string]UsageInspection, error) {
	result := make(map[string]UsageInspection)
	file, err := openCaptureEvidence(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	reader := &io.LimitedReader{R: file, N: maxCaptureEvidenceBytes + 1}
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 4096), 32<<20)
	seen := make(map[string]bool)
	for line := 1; scanner.Scan(); line++ {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		var record captureRecord
		if !utf8.Valid(scanner.Bytes()) || json.Unmarshal(scanner.Bytes(), &record, jsonv1.DefaultOptionsV1()) != nil {
			return nil, fmt.Errorf("capture line %d: invalid record", line)
		}
		if record.SchemaVersion != schemaVersion || record.CapturedAt.IsZero() {
			return nil, fmt.Errorf("capture line %d: invalid record or unsupported schema", line)
		}
		if record.Boundary != "provider" && record.Boundary != "native_response" {
			continue
		}
		if record.CaptureID == "" || record.Boundary == "provider" && record.ProviderAttempt == 0 {
			return nil, fmt.Errorf("capture line %d: missing provider identity", line)
		}
		key := fmt.Sprintf("%s/%s/%d", record.ThreadID, record.CaptureID, record.ProviderAttempt)
		if seen[key] {
			return nil, fmt.Errorf("capture line %d: duplicate provider attempt", line)
		}
		seen[key] = true
		if !filter.accepts(record.ThreadID, record.RequestModel, record.CapturedAt) {
			continue
		}
		entry := result[record.ThreadID]
		valid := record.Usage != nil && record.Usage.EvidenceComplete && record.ResponseComplete && record.CaptureError == ""
		var raw struct {
			Usage map[string]jsonv1.RawMessage `json:"usage"`
		}
		if err := json.Unmarshal(scanner.Bytes(), &raw, jsonv1.DefaultOptionsV1()); err != nil {
			return nil, err
		}
		for _, field := range []string{"input_tokens", "cached_input_tokens", "output_tokens", "reasoning_tokens"} {
			if len(raw.Usage[field]) == 0 || string(raw.Usage[field]) == "null" {
				valid = false
			}
		}
		if err := entry.add(valid, record.Usage); err != nil {
			return nil, err
		}
		result[record.ThreadID] = entry
	}
	if err := scanner.Err(); err != nil {
		return nil, err
	}
	if reader.N == 0 {
		return nil, errors.New("capture exceeds evidence size limit")
	}
	return result, nil
}

const maxCaptureEvidenceBytes = 64 << 20

func openCaptureEvidence(path string) (*os.File, error) {
	file, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NONBLOCK, 0)
	if err != nil {
		return nil, err
	}
	info, err := file.Stat()
	if err != nil || !info.Mode().IsRegular() || info.Size() > maxCaptureEvidenceBytes {
		file.Close()
		return nil, errors.New("evidence must be a bounded regular file")
	}
	return file, nil
}
