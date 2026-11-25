package launch

import (
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

type headlessCapture struct {
	mu        sync.Mutex
	file      *os.File
	completed uint64
	failed    uint64
	usage     inference.Usage
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
	c.completed++
	c.usage.InputTokens += counts.InputTokens
	c.usage.UncachedInputTokens += counts.UncachedInputTokens
	c.usage.CacheWriteTokens += counts.CacheWriteTokens
	c.usage.OutputTokens += counts.OutputTokens
	c.usage.ReasoningTokens += counts.ReasoningTokens
	if c.file != nil {
		record := map[string]any{
			"event":       "response_completed",
			"thread_id":   threadID,
			"turn_id":     turnID,
			"model":       model,
			"duration_ms": duration.Milliseconds(),
			"usage":       counts,
		}
		if data, err := json.Marshal(record); err == nil {
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
	c.failed++
	if c.file != nil {
		record := map[string]any{
			"event":     "response_failed",
			"thread_id": threadID,
			"turn_id":   turnID,
		}
		if data, err := json.Marshal(record); err == nil {
			_, _ = c.file.Write(append(data, '\n'))
		}
	}
}

func (c *headlessCapture) WriteMetrics(path string) error {
	if c == nil || path == "" {
		return nil
	}
	c.mu.Lock()
	payload := map[string]any{
		"completed": c.completed,
		"failed":    c.failed,
		"usage":     c.usage,
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
