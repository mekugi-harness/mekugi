//go:build e2e

package launch

import (
	"bufio"
	"bytes"
	"cmp"
	"encoding/binary"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

func liveOpenAIEnvironment(t *testing.T, traceRoot string) []string {
	t.Helper()
	home := os.Getenv("CODEX_HOME")
	if home == "" {
		user, err := os.UserHomeDir()
		if err != nil {
			t.Fatal(err)
		}
		home = filepath.Join(user, ".codex")
	}
	auth, err := os.ReadFile(filepath.Join(home, "auth.json"))
	if err != nil {
		t.Fatal("live Codex authentication required")
	}
	environment := slices.DeleteFunc(routerFaultCodexEnvironment(t), func(entry string) bool {
		key, _, _ := strings.Cut(entry, "=")
		switch key {
		case "OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY", "CHATGPT_API_KEY", "CODEX_ROLLOUT_TRACE_ROOT":
			return true
		}
		return false
	})
	for _, entry := range environment {
		if path, ok := strings.CutPrefix(entry, "CODEX_HOME="); ok {
			if err := os.WriteFile(filepath.Join(path, "auth.json"), auth, 0600); err != nil {
				t.Fatal(err)
			}
		}
	}
	return append(environment, "CODEX_ROLLOUT_TRACE_ROOT="+traceRoot)
}

type liveInference struct {
	Thread                    string
	Turn                      string
	Provider                  string
	At                        int64
	Fields                    map[string]jsontext.Value
	Input                     []jsontext.Value
	Output                    []jsontext.Value
	InputTokens, CachedTokens uint64
}

func (s liveInference) contains(text string) bool {
	return slices.ContainsFunc(s.Input, func(item jsontext.Value) bool {
		return bytes.Contains(item, []byte(text))
	})
}

type livePayloadRef struct {
	ID      string           `json:"raw_payload_id"`
	Shard   int              `json:"shard"`
	Parts   []jsontext.Value `json:"parts"`
	Capture jsontext.Value   `json:"capture"`
}

// Read completed native attempts after the process exits. The trace observes
// requests and token usage without altering the real provider connection.
func readLiveInferences(t *testing.T, traceRoot string) []liveInference {
	t.Helper()
	entries, err := os.ReadDir(traceRoot)
	if err != nil {
		t.Fatal(err)
	}
	var samples []liveInference
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		bundle := filepath.Join(traceRoot, entry.Name())
		samples = append(samples, readLiveInferenceBundle(t, bundle)...)
	}
	if len(samples) == 0 {
		t.Fatal("native trace did not capture completed inference")
	}
	slices.SortStableFunc(samples, func(a, b liveInference) int { return cmp.Compare(a.At, b.At) })
	return samples
}

func readLiveInferenceBundle(t *testing.T, bundle string) []liveInference {
	t.Helper()
	shards := make(map[int]map[string][]byte)
	load := func(ref livePayloadRef, dst any) {
		t.Helper()
		if ref.Shard == 0 || ref.ID == "" || len(ref.Capture) != 0 || len(ref.Parts) != 0 {
			t.Fatal("native inference payload was not captured in one complete record")
		}
		if shards[ref.Shard] == nil {
			shards[ref.Shard] = readLivePayloadShard(t, filepath.Join(bundle, "payloads", fmt.Sprintf("%06d.zst", ref.Shard)))
		}
		if err := json.Unmarshal(shards[ref.Shard][ref.ID], dst); err != nil {
			t.Fatalf("invalid native inference payload %s: %v", ref.ID, err)
		}
	}
	file, err := os.Open(filepath.Join(bundle, "trace.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()

	started := make(map[string]liveInference)
	completed := make(map[string]liveInference)
	var samples []liveInference
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 64<<10), 8<<20)
	for scanner.Scan() {
		var row struct {
			Thread  string `json:"thread_id"`
			Turn    string `json:"codex_turn_id"`
			At      int64  `json:"wall_time_unix_ms"`
			Payload struct {
				Type       string         `json:"type"`
				ID         string         `json:"inference_call_id"`
				Provider   string         `json:"provider_name"`
				ResponseID string         `json:"response_id"`
				Request    livePayloadRef `json:"request_payload"`
				Warmup     livePayloadRef `json:"warmup_context_payload"`
				Response   livePayloadRef `json:"response_payload"`
			} `json:"payload"`
		}
		if err := json.Unmarshal(scanner.Bytes(), &row); err != nil {
			t.Fatalf("invalid native trace row: %v", err)
		}
		p := row.Payload
		switch p.Type {
		case "inference_started":
			sample := liveInference{Thread: row.Thread, Turn: row.Turn, At: row.At, Provider: p.Provider}
			load(p.Request, &sample.Fields)
			var input struct {
				Items []jsontext.Value `json:"input"`
			}
			if p.Warmup.ID != "" {
				load(p.Warmup, &input)
			} else {
				if err := json.Unmarshal(sample.Fields["input"], &input.Items); err != nil {
					t.Fatal("native request omitted its input")
				}
				if raw := sample.Fields["previous_response_id"]; len(raw) > 0 && string(raw) != "null" {
					var id string
					if err := json.Unmarshal(raw, &id); err != nil {
						t.Fatal("invalid native continuation identity")
					}
					previous, ok := completed[id]
					if !ok || previous.Thread != sample.Thread {
						t.Fatal("native continuation omitted its previous response")
					}
					input.Items = slices.Concat(previous.Input, previous.Output, input.Items)
				}
			}
			sample.Input = input.Items
			started[p.ID] = sample
		case "inference_completed":
			sample, ok := started[p.ID]
			if !ok {
				t.Fatal("native completion omitted its inference request")
			}
			delete(started, p.ID)
			var result struct {
				Output []jsontext.Value `json:"output_items"`
				Usage  struct {
					Input  uint64 `json:"input_tokens"`
					Cached uint64 `json:"cached_input_tokens"`
				} `json:"token_usage"`
			}
			load(p.Response, &result)
			if result.Usage.Cached > result.Usage.Input {
				t.Fatal("native inference returned more cached tokens than input tokens")
			}
			sample.Output = result.Output
			sample.InputTokens, sample.CachedTokens = result.Usage.Input, result.Usage.Cached
			completed[p.ResponseID] = sample
			samples = append(samples, sample)
		case "inference_failed", "inference_cancelled":
			delete(started, p.ID)
		}
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	return samples
}

func readLivePayloadShard(t *testing.T, path string) map[string][]byte {
	t.Helper()
	compressed, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer compressed.Close()
	decoder, err := zstd.NewReader(compressed, zstd.WithDecoderConcurrency(1))
	if err != nil {
		t.Fatal(err)
	}
	defer decoder.Close()
	// Trace events reference material already flushed to the shard. A process
	// can shut down before sealing its last Zstandard frame.
	body, err := io.ReadAll(decoder)
	if err != nil && !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatal(err)
	}
	records := make(map[string][]byte)
	// codex-rs/rollout-trace/src/shard.rs: each record is an ID length, body
	// length, then exact ID and body. Both lengths are little-endian.
	for len(body) >= 10 {
		start := 10 + int(binary.LittleEndian.Uint16(body[:2]))
		size := binary.LittleEndian.Uint64(body[2:10])
		if start > len(body) || size > uint64(len(body)-start) {
			break
		}
		end := start + int(size)
		records[string(body[10:start])] = body[start:end]
		body = body[end:]
	}
	return records
}
