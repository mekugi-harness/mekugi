//go:build codex_e2e || e2e

package launch

import (
	"context"
	jsonv1 "encoding/json"
	json "encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"os"
	"slices"
	"strings"
	"testing"

	"github.com/coder/websocket"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
)

const (
	fixtureMaxResponseRequestBytes = 32 << 20
	fixtureThreadIDHeader          = "thread-id"
	fixtureTurnMetadataHeader      = "x-codex-turn-metadata"
)

type fixtureRequestMetadata struct {
	Model           string                       `json:"model"`
	Effort          string                       `json:"reasoning_effort"`
	ContextWindowID string                       `json:"context_window_id"`
	WindowNumber    uint64                       `json:"window_number"`
	ThreadID        string                       `json:"thread_id"`
	RequestKind     string                       `json:"request_kind"`
	TurnID          string                       `json:"turn_id"`
	Directories     map[string]jsonv1.RawMessage `json:"workspaces"`
}

func fixtureTurnMetadata(headers http.Header) (fixtureRequestMetadata, bool) {
	var metadata fixtureRequestMetadata
	err := json.Unmarshal([]byte(headers.Get(fixtureTurnMetadataHeader)), &metadata)
	return metadata, err == nil
}

type fixtureResponsesProvider interface {
	respond(context.Context, []byte, http.Header) (*http.Response, error)
}

// This is the local provider, not a relay. Nothing is rewritten or forwarded.
func fixtureResponsesHandler(provider fixtureResponsesProvider) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/v1/responses" {
			http.NotFound(w, r)
			return
		}
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		response, err := provider.respond(r.Context(), body, r.Header)
		if err != nil {
			if r.Context().Err() == nil {
				http.Error(w, err.Error(), http.StatusBadRequest)
			}
			return
		}
		defer response.Body.Close()
		for key, values := range response.Header {
			w.Header()[key] = values
		}
		w.WriteHeader(response.StatusCode)
		_, _ = io.Copy(fixtureStreamWriter{w}, response.Body)
	}
}

type fixtureStreamWriter struct{ http.ResponseWriter }

func (w fixtureStreamWriter) Write(body []byte) (int, error) {
	n, err := w.ResponseWriter.Write(body)
	if err != nil {
		return n, err
	}
	return n, http.NewResponseController(w.ResponseWriter).Flush()
}

func fixtureProviderArgs(endpoint string, args []string, providerID string) []string {
	if providerID == "" || providerID == "openai" {
		return appendInvocationOverrides(args, "-c", fmt.Sprintf("openai_base_url=%q", endpoint))
	}
	return appendInvocationOverrides(args, "-c", fmt.Sprintf("model_providers.%q.base_url=%q", providerID, endpoint))
}

func appendInvocationOverrides(args []string, overrides ...string) []string {
	index := slices.Index(args, "--")
	if index < 0 {
		index = len(args)
	}
	return slices.Insert(slices.Clone(args), index, overrides...)
}

func fixturePromptCacheKey(body []byte) string {
	var request struct {
		Key string `json:"prompt_cache_key"`
	}
	_ = json.Unmarshal(body, &request)
	return request.Key
}

func fixtureResponsesSSE(events ...any) string {
	var stream strings.Builder
	for _, event := range events {
		stream.WriteString("data: ")
		stream.Write(responses.MustMarshalJSON(event))
		stream.WriteString("\n\n")
	}
	return stream.String()
}

func fixtureResponsesStream(wire string) *http.Response {
	return &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": {"text/event-stream"}},
		Body:       io.NopCloser(strings.NewReader(wire)),
	}
}

func writeFixtureResponseSocket(ctx context.Context, conn *websocket.Conn, value any) error {
	return conn.Write(ctx, websocket.MessageText, responses.MustMarshalJSON(value))
}

func readFixtureResponseSocket(ctx context.Context, conn *websocket.Conn) (map[string]jsonv1.RawMessage, error) {
	// The full instructions and tool catalog can exceed the library's default.
	conn.SetReadLimit(fixtureMaxResponseRequestBytes)
	_, body, err := conn.Read(ctx)
	if err != nil {
		return nil, err
	}
	var fields map[string]jsonv1.RawMessage
	err = json.Unmarshal(body, &fields)
	return fields, err
}

func fixtureSocketEvent(kind, id string) map[string]any {
	return map[string]any{"type": kind, "response": map[string]any{"id": id, "status": strings.TrimPrefix(kind, "response."), "output": []any{}}}
}

func fixtureResponsesSuccess() *http.Response {
	const answer = "Recovered after a retry."
	item := map[string]any{
		"type": "message", "id": "retry-answer", "role": "assistant", "status": "completed",
		"content": []any{map[string]any{"type": "output_text", "text": answer, "annotations": []any{}}},
	}
	return fixtureResponsesStream(fixtureResponsesSSE(
		map[string]any{"type": "response.created", "response": map[string]any{"id": "retry-response", "status": "in_progress", "output": []any{}}},
		map[string]any{"type": "response.output_item.added", "output_index": 0, "item": map[string]any{
			"type": "message", "id": "retry-answer", "role": "assistant", "status": "in_progress", "content": []any{},
		}},
		map[string]any{"type": "response.content_part.added", "item_id": "retry-answer", "output_index": 0, "content_index": 0, "part": map[string]any{"type": "output_text", "text": ""}},
		map[string]any{"type": "response.output_text.delta", "item_id": "retry-answer", "output_index": 0, "content_index": 0, "delta": answer},
		map[string]any{"type": "response.output_text.done", "item_id": "retry-answer", "output_index": 0, "content_index": 0, "text": answer},
		map[string]any{"type": "response.content_part.done", "item_id": "retry-answer", "output_index": 0, "content_index": 0, "part": map[string]any{"type": "output_text", "text": answer, "annotations": []any{}}},
		map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item},
		map[string]any{"type": "response.completed", "response": map[string]any{"id": "retry-response", "status": "completed", "output": []any{item}, "usage": map[string]any{"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}},
	))
}

func routerFaultCodexEnvironment(t *testing.T) []string {
	t.Helper()
	configRoot := t.TempDir()
	environment := make([]string, 0, len(os.Environ())+4)
	for _, entry := range os.Environ() {
		key, _, _ := strings.Cut(entry, "=")
		switch key {
		case "HOME", "ZDOTDIR", "CODEX_HOME", "XDG_CONFIG_HOME", "BASH_ENV", "TMPDIR", "CODEX_ARG0_PATH_DIR", "MEKUGI_AUTO_TITLE":
			continue
		case "PATH":
			entry = "PATH=" + execTrackPath()
		}
		environment = append(environment, entry)
	}
	// Release builds stage helper executables only outside their temp root.
	return append(environment, "HOME="+configRoot, "CODEX_HOME="+configRoot, "XDG_CONFIG_HOME="+configRoot, "TMPDIR="+t.TempDir())
}
