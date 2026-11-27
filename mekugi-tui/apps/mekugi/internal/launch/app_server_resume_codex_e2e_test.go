//go:build codex_e2e

package launch

import (
	"bytes"
	"context"
	json "encoding/json/v2"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/charmbracelet/x/vt"
	"golang.org/x/term"
)

type appResumeProvider struct {
	mu       sync.Mutex
	threads  []string
	settings []appResumeInferenceSettings
}

type appResumeInferenceSettings struct {
	Model     string `json:"model"`
	Tier      string `json:"service_tier"`
	Reasoning struct {
		Effort string `json:"effort"`
	} `json:"reasoning"`
}

func (p *appResumeProvider) respond(_ context.Context, body []byte, headers http.Header) (*http.Response, error) {
	var settings appResumeInferenceSettings
	if err := json.Unmarshal(body, &settings); err != nil {
		return nil, err
	}
	thread := headers.Get(fixtureThreadIDHeader)
	if thread == "" {
		metadata, _ := fixtureTurnMetadata(headers)
		thread = metadata.ThreadID
	}
	p.mu.Lock()
	p.threads = append(p.threads, thread)
	p.settings = append(p.settings, settings)
	p.mu.Unlock()
	return fixtureResponsesSuccess(), nil
}

func (p *appResumeProvider) assertSettings(t *testing.T, index int, model, effort, tier string) {
	t.Helper()
	p.mu.Lock()
	defer p.mu.Unlock()
	if index >= len(p.settings) {
		t.Fatalf("missing inference %d: %d requests", index, len(p.settings))
	}
	got := p.settings[index]
	if got.Model != model || got.Reasoning.Effort != effort || got.Tier != tier {
		t.Fatalf("inference %d settings: %+v, want %s/%s/%s", index, got, model, effort, tier)
	}
}

func (p *appResumeProvider) snapshot() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.threads...)
}

func TestAppServerResumeNativeCodex(t *testing.T) {
	t.Run("settings", testAppServerResumeSettingsNativeCodex)
	t.Run("picker", testAppServerResumePickerNativeCodex)
}

// Runs fresh managed processes to exercise native persistence and settings
// precedence, including an idle setting change that does not sample.
func testAppServerResumeSettingsNativeCodex(t *testing.T) {
	codex, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	provider := &appResumeProvider{}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()
	t.Setenv("XDG_STATE_HOME", t.TempDir()) // State belongs to the parent UI, not just its Codex child.
	environment := routerFaultCodexEnvironment(t)
	workspace := t.TempDir()
	const savedProvider = "savedpreview"
	providerName := savedProvider
	providerConfig := `{name="preview",base_url=` + strconv.Quote(server.URL+"/v1") + `,wire_api="responses",requires_openai_auth=false}`
	newCommand := func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex,
			"-c", "model_providers."+savedProvider+"="+providerConfig,
			"-c", "model_providers.preview="+providerConfig,
			"-c", "model_provider="+strconv.Quote(providerName),
			"-c", "features.plugins=false")
		cmd.Env = environment
		cmd.Dir = workspace
		return cmd
	}

	first := startAppResumeTerminalWithIntegration(t, newCommand, "", appServerIntegration{
		launchArgs: []string{"-m", "gpt-6-sol", "-c", `model_reasoning_effort="high"`, "-c", `service_tier="flex"`},
	})
	first.awaitReady()
	first.await("gpt-6-sol (high) · flex")
	first.send("First resume question\r")
	first.await("Recovered after a retry.")
	threads := provider.snapshot()
	if len(threads) != 1 || threads[0] == "" {
		t.Fatalf("first turn identities: %q", threads)
	}
	thread := threads[0]
	first.awaitThreadIdle(thread)
	first.send("/tier priority\r") // Idle changes after the last turn must also restore.
	first.await("gpt-6-sol (high) · priority")
	first.send("\x022") // Focus Diff, then widen Main before persisting this UI state.
	first.await("s files")
	first.send("\x02l")
	first.awaitMatch("resized split", func(screen string) bool {
		return appResumeSplit(screen) > 50 && strings.Contains(screen, "s files")
	})
	savedSplit := appResumeSplit(first.screen.String())
	first.stopCanceled()
	provider.assertSettings(t, 0, "gpt-6-sol", "high", "flex")
	// Cold resume tests the native owner's durable settings through an actual
	// provider request, independent of the persistence record format.
	providerName = "preview"
	second := startAppResumeTerminal(t, newCommand, "--last")
	second.awaitReady()
	second.await("gpt-6-sol (high)") // Retained usage can crowd the tier out of the narrow footer.
	second.await("First resume question")
	second.await("Recovered after a retry.")
	second.awaitMatch("restored Diff focus and split", func(screen string) bool {
		return strings.Contains(screen, "s files") && appResumeSplit(screen) == savedSplit
	})
	if got := provider.snapshot(); len(got) != 1 {
		t.Fatalf("resume resent a turn before new input: %q", got)
	}
	second.send("\x021")
	second.send("Second resume question\r")
	second.await("Second resume question")
	second.awaitThreadIdle(thread)
	second.quit()
	threads = provider.snapshot()
	provider.assertSettings(t, 1, "gpt-6-sol", "high", "priority")
	if len(threads) != 2 || threads[1] != thread {
		t.Fatalf("turns did not retain one thread with no duplicate submission: %q", threads)
	}
	third := startAppResumeTerminalWithIntegration(t, newCommand, thread, appServerIntegration{
		launchArgs: []string{"-m", "gpt-6-astra"},
	})
	third.awaitReady()
	third.await("gpt-6-astra (high)")
	third.send("Explicit model keeps saved effort and tier\r")
	third.await("Explicit model keeps saved effort and tier")
	third.awaitThreadIdle(thread)
	third.quit()
	provider.assertSettings(t, 2, "gpt-6-astra", "high", "priority")
	fourth := startAppResumeTerminalWithIntegration(t, newCommand, thread, appServerIntegration{
		launchArgs: []string{"-c", `model_reasoning_effort="low"`, "-c", `service_tier="flex"`},
	})
	fourth.awaitReady()
	fourth.await("gpt-6-astra (low)")
	fourth.send("Explicit effort and tier keep saved model\r")
	fourth.await("Explicit effort and tier keep saved model")
	fourth.awaitThreadIdle(thread)
	fourth.quit()
	provider.assertSettings(t, 3, "gpt-6-astra", "low", "flex")
	if got := provider.snapshot(); len(got) != 4 {
		t.Fatalf("resume resent an unrequested turn: %q", got)
	}
}

// The top row's divider is the right pane's origin in a 100-column PTY.
func appResumeSplit(screen string) int {
	row, _, _ := strings.Cut(screen, "\n")
	for i, r := range []rune(row) {
		if r == '│' && i > 0 {
			return i
		}
	}
	return -1
}

type appResumeTerminal struct {
	t      *testing.T
	ctx    context.Context
	cancel context.CancelFunc
	outer  *os.File
	inner  *os.File
	frames chan []byte
	done   chan error
	screen *vt.Emulator
	before *term.State
	socket string
}

func startAppResumeTerminal(t *testing.T, newCommand func(context.Context) *exec.Cmd, resumeThread string) *appResumeTerminal {
	return startAppResumeTerminalWithIntegration(t, newCommand, resumeThread, appServerIntegration{})
}

func startAppResumeTerminalWithIntegration(t *testing.T, newCommand func(context.Context) *exec.Cmd, resumeThread string, integration appServerIntegration) *appResumeTerminal {
	t.Helper()
	return startAppResumeTerminalWithDirectNative(t, newCommand, resumeThread, integration)
}

func observeAppResumeTerminal(t *testing.T, ctx context.Context, cancel context.CancelFunc, outer, inner *os.File, before *term.State, wait func() error) *appResumeTerminal {
	t.Helper()
	s := &appResumeTerminal{t: t, ctx: ctx, cancel: cancel, outer: outer, inner: inner,
		frames: make(chan []byte, 32), done: make(chan error, 1), screen: vt.NewEmulator(100, 30), before: before}
	stopReplies := startEmulatorReplies(outer, s.screen)
	waiterDone := make(chan struct{})
	go func() { defer close(waiterDone); s.done <- wait() }()
	go func() {
		defer close(s.frames)
		buf := make([]byte, 65536)
		for {
			n, err := outer.Read(buf)
			if n > 0 {
				select {
				case s.frames <- bytes.Clone(buf[:n]):
				case <-ctx.Done():
					return
				}
			}
			if err != nil {
				return
			}
		}
	}()
	t.Cleanup(func() {
		cancel()
		outer.Close()
		inner.Close()
		<-waiterDone
		for range s.frames {
		}
		stopReplies()
	})
	return s
}

func (s *appResumeTerminal) await(needle string) {
	s.t.Helper()
	s.awaitMatch(needle, func(content string) bool {
		if needle == "completed" {
			return composerStatusShows(content, "Completed")
		}
		return strings.Contains(content, needle)
	})
}

func (s *appResumeTerminal) awaitReady() {
	s.t.Helper()
	s.awaitMatch("available Main composer", func(content string) bool {
		return strings.Contains(content, "\n❯") && strings.Contains(content, "Main • ") &&
			!composerStatusShows(content, "Working") && !composerStatusShows(content, "Sending")
	})
}

// awaitStatus waits for the composer's status row to start with status.
func (s *appResumeTerminal) awaitStatus(status string) {
	s.t.Helper()
	s.awaitMatch("status "+status, func(content string) bool { return composerStatusShows(content, status) })
}

// Wait for the native owner to settle before performing an idle-only action.
// Callers first observe the request or its terminal response.
func (s *appResumeTerminal) awaitThreadIdle(thread string) {
	s.t.Helper()
	s.awaitThreadStatus(thread, "idle")
}

func (s *appResumeTerminal) awaitThreadStatus(thread, status string) {
	s.t.Helper()
	wire, err := dialNativeAppServer(s.ctx, s.socket)
	if err != nil {
		s.t.Fatal(err)
	}
	defer wire.Close()
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
		if err := wire.Call(s.ctx, "thread/read", map[string]any{"threadId": thread, "includeTurns": false}, &result); err != nil {
			s.t.Fatal(err)
		}
		if result.Thread.Status.Type == status {
			return
		}
		select {
		case <-ticker.C:
		case <-s.ctx.Done():
			s.t.Fatalf("thread %s did not become %s: %v", thread, status, s.ctx.Err())
		}
	}
}

func (s *appResumeTerminal) awaitMatch(label string, visible func(string) bool) {
	s.t.Helper()
	for !visible(s.screen.String()) {
		select {
		case frame, ok := <-s.frames:
			if !ok {
				s.t.Fatalf("terminal closed before %q\n%s", label, s.screen.String())
			}
			s.screen.Write(frame)
		case err := <-s.done:
			s.t.Fatalf("UI exited before %q: %v\n%s", label, err, s.screen.String())
		case <-s.ctx.Done():
			s.t.Fatalf("missing %q: %v\n%s", label, s.ctx.Err(), s.screen.String())
		}
	}
}

func (s *appResumeTerminal) stopCanceled() {
	s.t.Helper()
	s.cancel()
	select {
	case err := <-s.done:
		if !errors.Is(err, context.Canceled) {
			s.t.Fatalf("canceled UI returned %v, want context.Canceled", err)
		}
	case <-time.After(10 * time.Second):
		s.t.Fatal("canceled UI did not shut down")
	}
	s.checkRestored()
}

func (s *appResumeTerminal) send(input string) {
	s.t.Helper()
	if _, err := io.WriteString(s.outer, input); err != nil {
		s.t.Fatal(err)
	}
}

func (s *appResumeTerminal) quit() {
	s.t.Helper()
	s.send("/quit\r")
	frames := s.frames
	for {
		select {
		case err := <-s.done:
			if err != nil {
				s.t.Fatal(err)
			}
			s.checkRestored()
			return
		case frame, ok := <-frames:
			if !ok {
				frames = nil
				continue
			}
			s.screen.Write(frame)
			if strings.Contains(s.screen.String(), "Quit this session?") {
				s.t.Fatalf("quit unexpectedly offered running work\n%s", s.screen.String())
			}
		case <-s.ctx.Done():
			s.t.Fatalf("quit: %v\n%s", s.ctx.Err(), s.screen.String())
		}
	}
}

func (s *appResumeTerminal) checkRestored() {
	s.t.Helper()
	after, err := term.GetState(int(s.inner.Fd()))
	if err != nil {
		s.t.Fatal(err)
	}
	if !reflect.DeepEqual(s.before, after) {
		s.t.Fatal("terminal mode not restored")
	}
	s.cancel()
	s.outer.Close()
	s.inner.Close()
}

func TestAppServerOptionWordNavigationNativeCodex(t *testing.T) {
	codex, err := exec.LookPath("codex")
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	environment := routerFaultCodexEnvironment(t)
	workspace := t.TempDir()
	provider := &appResumeProvider{}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()
	terminal := startAppResumeTerminal(t, func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex, "app-server",
			"-c", `model_providers.keys={name="keys",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false}`,
			"-c", `model_provider="keys"`, "-c", `model="gpt-6-astra"`)
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}, "")
	terminal.awaitReady()
	terminal.send("one two three\x1bb\x1bbX\x1bfY")
	terminal.await("one Xtwo Ythree")
	terminal.send("\x03")
	terminal.send("alpha beta gamma\x1b[1;3D\x1b[1;3DX\x1b[1;3CY")
	terminal.await("alpha Xbeta Ygamma")
	terminal.send("\x03")
	terminal.quit()
	if got := provider.snapshot(); len(got) != 0 {
		t.Fatalf("word navigation submitted a model request: %q", got)
	}
}

// The picker lists Mekugi's saved sessions, switches the running UI
// to one, and serves a bare startup resume. Provider thread identities prove
// where later input went.
func testAppServerResumePickerNativeCodex(t *testing.T) {
	codex, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	provider := &appResumeProvider{}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	environment := routerFaultCodexEnvironment(t)
	workspace := t.TempDir()
	newCommand := func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex,
			"-c", `model_providers.pickerpreview={name="preview",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false}`,
			"-c", `model_provider="pickerpreview"`, "-c", `model="gpt-6-astra"`,
			"-c", "features.plugins=false")
		cmd.Env = environment
		cmd.Dir = workspace
		return cmd
	}

	first := startAppResumeTerminal(t, newCommand, "")
	first.awaitReady()
	first.send("Picker first question\r")
	first.await("Recovered after a retry.")
	first.awaitThreadIdle(provider.snapshot()[0])
	first.quit()

	second := startAppResumeTerminal(t, newCommand, "")
	second.awaitReady()
	second.send("Picker second question\r")
	second.await("Recovered after a retry.")
	second.awaitThreadIdle(provider.snapshot()[1])
	second.send("/resume\r")
	second.await("Resume a previous session")
	second.awaitMatch("both saved sessions", func(screen string) bool {
		return strings.Contains(screen, "Picker first question") && strings.Contains(screen, "Picker second question · current")
	})
	second.send("\x1b[B\r")
	second.awaitMatch("switched transcript", func(screen string) bool {
		return strings.Contains(screen, "Picker first question") && !strings.Contains(screen, "Picker second question") && !strings.Contains(screen, "Resume a previous session")
	})
	second.awaitReady()
	second.send("Picker follow-up\r")
	second.await("Picker follow-up")
	second.awaitThreadIdle(provider.snapshot()[0])
	second.quit()
	threads := provider.snapshot()
	if len(threads) != 3 || threads[0] == threads[1] || threads[2] != threads[0] {
		t.Fatalf("switched input did not reach the chosen thread: %q", threads)
	}

	third := startAppResumeTerminal(t, newCommand, "--pick")
	third.await("Resume a previous session")
	third.await("esc start new")
	// The follow-up made the first session the most recently updated.
	third.awaitMatch("recent session first", func(screen string) bool {
		first, second := strings.Index(screen, "Picker first question"), strings.Index(screen, "Picker second question")
		return first >= 0 && second > first
	})
	third.send("\r")
	third.await("Picker follow-up")
	third.awaitReady()
	third.quit()
	if got := provider.snapshot(); len(got) != 3 {
		t.Fatalf("startup picker resent a turn: %q", got)
	}
}
