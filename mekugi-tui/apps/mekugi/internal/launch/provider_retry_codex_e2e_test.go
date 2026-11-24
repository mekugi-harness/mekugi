//go:build codex_e2e

package launch

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"io"
	"net/http"
	"net/http/httptest"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

type retryFixtureProvider struct {
	mu       sync.Mutex
	requests int
	drop     bool
}

func (p *retryFixtureProvider) respond(_ context.Context, _ []byte, _ http.Header) (*http.Response, error) {
	p.mu.Lock()
	p.requests++
	request := p.requests
	p.mu.Unlock()
	if request != 1 {
		return fixtureResponsesSuccess(), nil
	}
	if p.drop {
		return fixtureResponsesStream(fixtureResponsesSSE(map[string]any{
			"type": "response.created", "response": map[string]any{"id": "closed-response", "status": "in_progress"},
		})), nil
	}
	return &http.Response{
		StatusCode: http.StatusServiceUnavailable,
		Header:     http.Header{"Content-Type": {"application/json"}, "Retry-After": {"0"}},
		Body:       io.NopCloser(strings.NewReader(`{"error":{"code":"server_error","message":"temporary upstream failure"}}`)),
	}, nil
}

func runRetryFixtureNativeCodex(t *testing.T, serverURL, prompt string) (stdout, stderr string, runErr error) {
	t.Helper()
	codex, err := exec.LookPath("codex")
	if err != nil {
		t.Fatal("native Codex is required for the direct provider retry acceptance gate")
	}
	config := `model_providers.retry_fixture={name="retry_fixture",base_url=` + strconv.Quote(serverURL+"/v1") + `,wire_api="responses",requires_openai_auth=false}`
	ctx, cancel := context.WithTimeout(t.Context(), 90*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, codex,
		"-c", config, "-c", `model_provider="retry_fixture"`,
		"-c", "tools.update_plan.enabled=false",
		"--model", "gpt-6-astra", "--sandbox", "danger-full-access", "--ask-for-approval", "never",
		"exec", "--ignore-user-config", "--skip-git-repo-check", "--json", "--color", "never",
		"-C", t.TempDir(), prompt)
	cmd.Env = routerFaultCodexEnvironment(t)
	var output, errors bytes.Buffer
	cmd.Stdout, cmd.Stderr = &output, &errors
	runErr = cmd.Run()
	return output.String(), errors.String(), runErr
}

func retryFixtureNativeEventCount(t *testing.T, output, wantedType string) (count int, rawEvents []string) {
	t.Helper()
	for _, line := range strings.Split(output, "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		var event map[string]jsontext.Value
		if err := json.Unmarshal([]byte(line), &event); err != nil {
			t.Fatalf("invalid native Codex JSON event: %v\nline: %s", err, line)
		}
		var eventType string
		_ = json.Unmarshal(event["type"], &eventType)
		if eventType == wantedType {
			count++
			rawEvents = append(rawEvents, line)
		}
	}
	return count, rawEvents
}

func TestRecoveredProviderStreamNativeCodexE2E(t *testing.T) {
	provider := &retryFixtureProvider{drop: true}
	runAppServerPreview(t, provider)
	provider.mu.Lock()
	defer provider.mu.Unlock()
	if provider.requests != 2 {
		t.Fatalf("got %d provider requests, want a failed stream and one retry", provider.requests)
	}
}

func TestRetryablePrestream5xxStillRetriesInNativeCodexE2E(t *testing.T) {
	provider := &retryFixtureProvider{}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()

	stdout, stderr, runErr := runRetryFixtureNativeCodex(t, server.URL, "Complete after one retryable provider error.")
	if runErr != nil {
		t.Fatalf("native Codex did not recover from a retryable prestream 5xx: %v\nstdout: %.8000s\nstderr: %.8000s", runErr, stdout, stderr)
	}
	provider.mu.Lock()
	requests := provider.requests
	provider.mu.Unlock()
	if requests != 2 {
		t.Fatalf("native upstream requests = %d, want one 503 followed by one successful retry\nstdout: %.8000s\nstderr: %.8000s", requests, stdout, stderr)
	}
	completedCount, _ := retryFixtureNativeEventCount(t, stdout, "turn.completed")
	if completedCount != 1 || !strings.Contains(stdout, "Recovered after a retry.") {
		t.Fatalf("native Codex did not complete the retried turn: %s", stdout)
	}
}
