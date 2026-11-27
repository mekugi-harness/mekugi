//go:build codex_e2e

package launch

import (
	"bytes"
	"context"
	"net/http"
	"net/http/httptest"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"testing"
)

// appQueueProvider holds each gated inference until the test releases it.
type appQueueProvider struct {
	requests   chan []byte
	gates      []chan struct{}
	answers    []string
	mu         sync.Mutex
	count      int
	lastThread string
}

func (p *appQueueProvider) respond(ctx context.Context, body []byte, headers http.Header) (*http.Response, error) {
	p.mu.Lock()
	index := p.count
	p.count++
	metadata, _ := fixtureTurnMetadata(headers)
	p.lastThread = metadata.ThreadID
	p.mu.Unlock()
	p.requests <- bytes.Clone(body)
	if index < len(p.gates) {
		select {
		case <-p.gates[index]:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	if index < len(p.answers) {
		return fixtureTextResponse(strconv.Itoa(index), p.answers[index]), nil
	}
	return fixtureResponsesSuccess(), nil
}

func (p *appQueueProvider) thread() string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.lastThread
}

// Real Codex discards uncommitted steers on interrupt, commits accepted steers
// at the next sampling boundary, and starts queued input as its own turn.
func TestAppServerSteerAndQueueNativeCodex(t *testing.T) {
	codex, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	p := &appQueueProvider{requests: make(chan []byte, 8), gates: []chan struct{}{make(chan struct{}), make(chan struct{})}}
	server := httptest.NewServer(fixtureResponsesHandler(p))
	defer server.Close()
	defer func() {
		for _, gate := range p.gates {
			select {
			case <-gate:
			default:
				close(gate)
			}
		}
	}()
	environment, workspace := routerFaultCodexEnvironment(t), t.TempDir()
	terminal := startAppResumeTerminal(t, func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex, "-c", `model_providers.preview={name="Native control fixture",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false}`, "-c", `model_provider="preview"`, "-c", `model="mock-model"`, "-c", "features.plugins=false")
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}, "")
	next := func(label string) string {
		t.Helper()
		select {
		case request := <-p.requests:
			return string(request)
		case <-terminal.ctx.Done():
			t.Fatalf("no provider request for %s: %v\n%s", label, terminal.ctx.Err(), terminal.screen.String())
		}
		return ""
	}
	terminal.awaitReady()
	terminal.send("First blocked turn\r")
	next("first turn")
	thread := p.thread()
	terminal.awaitThreadStatus(thread, "active")

	terminal.send("steer interrupts\r")
	terminal.await("↳ steer interrupts")
	terminal.send("queued afterwards\t")
	terminal.await("↳ queued afterwards")
	terminal.send("\x03")
	terminal.awaitMatch("interrupted input restored", func(screen string) bool {
		return strings.Contains(screen, "❯ steer interrupts") && strings.Contains(screen, "queued afterwards") && !strings.Contains(screen, "↳")
	})
	terminal.awaitThreadIdle(thread)
	select {
	case request := <-p.requests:
		t.Fatalf("interruption automatically resent input: %s", request)
	default:
	}
	terminal.send("\r")
	resent := next("explicit resubmission")
	if strings.Count(resent, "steer interrupts") != 1 || !strings.Contains(resent, "queued afterwards") {
		t.Fatalf("restored input request: %s", resent)
	}
	terminal.awaitThreadStatus(thread, "active")
	terminal.send("queued later\t")
	terminal.await("↳ queued later")

	terminal.send("steer commits")
	terminal.await("❯ steer commits")
	terminal.send("\r")
	terminal.await("↳ steer commits")
	close(p.gates[1])
	if committed := next("committed steer"); strings.Count(committed, "steer commits") != 1 {
		t.Fatalf("steer not committed once: %s", committed)
	}
	if queued := next("queued turn"); strings.Count(queued, "queued later") != 1 || strings.Count(queued, "steer interrupts") != 1 {
		t.Fatalf("queued turn request: %s", queued)
	}
	terminal.awaitMatch("pending input cleared", func(screen string) bool {
		// Reply references also quote committed input; exact submission count is
		// checked in the provider request above, not across painted quotations.
		return !strings.Contains(screen, "↳")
	})
	terminal.awaitThreadIdle(thread)
	terminal.send("/compact\r")
	terminal.await("Context compacted")
	terminal.awaitThreadIdle(thread)
	select {
	case request := <-p.requests:
		t.Fatalf("manual reset unexpectedly sampled the model: %s", request)
	default:
	}
	terminal.send("/clear\r")
	terminal.awaitMatch("fresh session", func(screen string) bool {
		return strings.Contains(screen, "\n❯") && !strings.Contains(screen, "steer interrupts") && !strings.Contains(screen, "queued later")
	})
	terminal.send("Fresh session prompt\r")
	terminal.awaitMatch("fresh prompt admitted or kept", func(screen string) bool {
		return strings.Contains(screen, "❯ Fresh session prompt")
	})
	if strings.Contains(terminal.screen.String(), "Previous input is still settling") {
		t.Fatalf("fresh composer was displayed before the clear request could settle:\n%s", terminal.screen.String())
	}
	fresh := next("fresh session")
	freshThread := p.thread()
	if freshThread == thread || !strings.Contains(fresh, "Fresh session prompt") || strings.Contains(fresh, "steer interrupts") {
		t.Fatalf("clear retained prior context or thread: %s", fresh)
	}
	terminal.awaitThreadIdle(freshThread)
	terminal.quit()
}
