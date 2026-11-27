//go:build codex_e2e

package launch

import (
	"context"
	json "encoding/json/v2"
	"net/http/httptest"
	"os/exec"
	"strconv"
	"strings"
	"testing"
	"time"
)

// The pinned native host owns the active turn, manual compaction and next turn.
// Queue compaction during a blocked inference, then supply no more user input.
func TestAppServerQueuedCompactContinuesNativeCodex(t *testing.T) {
	native, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal("managed native app-server is required:", err)
	}
	const continuation = "Continuing after queued compaction."
	provider := &appQueueProvider{
		requests: make(chan []byte, 8),
		gates:    []chan struct{}{make(chan struct{})},
		answers:  []string{"Initial answer.", continuation},
	}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()
	defer func() {
		select {
		case <-provider.gates[0]:
		default:
			close(provider.gates[0])
		}
	}()
	environment, workspace := routerFaultCodexEnvironment(t), t.TempDir()
	control := make(chan string, 1)
	terminal := startAppResumeTerminalWithIntegration(t, func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, native, "-c", `model_providers.preview={name="queued-compact",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false}`, "-c", `model_provider="preview"`, "-c", `model="gpt-6-astra"`, "-c", "features.plugins=false")
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}, "", appServerIntegration{controlSocket: control})
	next := func(label string) string {
		t.Helper()
		select {
		case body := <-provider.requests:
			return string(body)
		case <-terminal.ctx.Done():
			t.Fatalf("no provider request for %s: %v\n%s", label, terminal.ctx.Err(), terminal.screen.String())
		}
		return ""
	}
	terminal.awaitReady()
	terminal.send("Work that will continue after queued compaction\r")
	if initial := next("initial task"); !strings.Contains(initial, "Work that will continue after queued compaction") {
		t.Fatal("provider did not receive the original task")
	}
	terminal.await("❯ Work that will continue after queued compaction")
	terminal.send("/compact\t")
	terminal.await("↳ /compact")
	select {
	case body := <-provider.requests:
		t.Fatalf("compaction replaced the active task: %s", body)
	default:
	}
	close(provider.gates[0])
	continued := next("automatic task continuation")
	var request struct {
		Input []struct {
			Role    string `json:"role"`
			Content []struct {
				Text string `json:"text"`
			} `json:"content"`
		} `json:"input"`
	}
	if err := json.Unmarshal([]byte(continued), &request); err != nil {
		t.Fatal(err)
	}
	for _, item := range request.Input {
		if item.Role != "user" {
			continue
		}
		for _, part := range item.Content {
			if strings.Contains(part.Text, "Continue the task from where") {
				t.Fatal("fabricated continuation user message")
			}
		}
	}
	terminal.await("Context compacted")
	terminal.await(continuation)
	awaitCompactIdle(t, terminal.ctx, <-control)
	terminal.quit()
	select {
	case body := <-provider.requests:
		t.Fatalf("continued more than once: %s", body)
	default:
	}
}

func awaitCompactIdle(t *testing.T, ctx context.Context, socket string) {
	t.Helper()
	wire, err := dialNativeAppServer(ctx, socket)
	if err != nil {
		t.Fatal(err)
	}
	defer wire.Close()
	var threads struct {
		Data []string `json:"data"`
	}
	if err := wire.Call(ctx, "thread/loaded/list", map[string]any{}, &threads); err != nil {
		t.Fatal(err)
	}
	if len(threads.Data) != 1 {
		t.Fatalf("expected one loaded thread, got %v", threads.Data)
	}
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	for {
		var result struct {
			Thread struct {
				Status struct {
					Type string `json:"type"`
				} `json:"status"`
			} `json:"thread"`
		}
		if err := wire.Call(ctx, "thread/read", map[string]any{"threadId": threads.Data[0], "includeTurns": false}, &result); err != nil {
			t.Fatal(err)
		}
		if result.Thread.Status.Type == "idle" {
			return
		}
		select {
		case <-ticker.C:
		case <-ctx.Done():
			t.Fatalf("compaction continuation did not settle: %v", ctx.Err())
		}
	}
}
