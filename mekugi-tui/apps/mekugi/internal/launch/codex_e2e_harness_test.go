//go:build codex_e2e

package launch

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"image"
	"image/png"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	vt "github.com/charmbracelet/x/vt"
	"github.com/creack/pty"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	"golang.org/x/term"
)

func e2eMustMarshalJSON(v any) []byte {
	data, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	return data
}

func e2eStringField(fields map[string]jsontext.Value, key string) string {
	raw, ok := fields[key]
	if !ok || len(raw) == 0 {
		return ""
	}
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s
	}
	return ""
}

// e2eRolloutResumeSettings scans a native rollout JSONL file for a thread's
// latest turn_context settings (model, reasoning effort, service_tier).
func e2eRolloutResumeSettings(threadID, path string) map[string]any {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()
	settings := map[string]any{}
	matchedThread := threadID == ""
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)
	for scanner.Scan() {
		var row struct {
			Type    string         `json:"type"`
			Payload jsontext.Value `json:"payload"`
		}
		if json.Unmarshal(scanner.Bytes(), &row) != nil {
			continue
		}
		switch row.Type {
		case "session_meta":
			var meta struct {
				ID string `json:"id"`
			}
			if json.Unmarshal(row.Payload, &meta) == nil && meta.ID == threadID {
				matchedThread = true
			}
		case "turn_context":
			var ctx struct {
				Model       string `json:"model"`
				Effort      string `json:"effort"`
				ServiceTier string `json:"service_tier"`
			}
			if json.Unmarshal(row.Payload, &ctx) == nil {
				if ctx.Model != "" {
					settings["model"] = ctx.Model
				}
				if ctx.Effort != "" {
					settings["model_reasoning_effort"] = ctx.Effort
				}
				if ctx.ServiceTier != "" {
					settings["service_tier"] = ctx.ServiceTier
				}
			}
		}
	}
	if !matchedThread {
		return nil
	}
	return settings
}

func mchangesNestedCodexResponse(turn int, item map[string]any) *http.Response {
	responseID := fmt.Sprintf("mchanges-nested-%d", turn)
	response := map[string]any{
		"id": responseID, "status": "completed", "output": []any{item},
		"usage": map[string]any{
			"input_tokens": 10, "input_tokens_details": map[string]any{"cached_tokens": 0},
			"output_tokens": 5, "output_tokens_details": map[string]any{"reasoning_tokens": 0}, "total_tokens": 15,
		},
	}
	var wire strings.Builder
	for _, event := range []map[string]any{
		{"type": "response.created", "response": map[string]any{"id": responseID, "status": "in_progress", "output": []any{}}},
		{"type": "response.output_item.added", "output_index": 0, "item": item},
		{"type": "response.output_item.done", "output_index": 0, "item": item},
		{"type": "response.completed", "response": response},
	} {
		wire.WriteString("data: ")
		wire.Write(e2eMustMarshalJSON(event))
		wire.WriteString("\n\n")
	}
	return fixtureResponsesStream(wire.String())
}

type observationFixture struct{}

type appServerIntegration struct {
	controlSocket chan<- string
	launchArgs    []string
}

func newManagedMekugiProxy(*testing.T) *observationFixture { return &observationFixture{} }

func attachTestReplayStore(*testing.T, *observationFixture) {}

func testAppServerIntegration(*observationFixture) appServerIntegration {
	return appServerIntegration{}
}

type appServerPreviewHook func(t *testing.T, outer io.Writer, await func(string), awaitFrame func(func(string) bool), screen *vt.Emulator)

type appServerPreview struct {
	managedRuntime            bool
	integration               *appServerIntegration
	environment               []string
	codexArgs                 []string
	completedText             string
	workspace                 string
	approvals                 bool
	saveBeforePrompt          bool
	beforePrompt, afterPrompt appServerPreviewHook
	duringTurn                appServerPreviewHook
}

const (
	browserWarmupPrompt = "Prepare the browser fixture."
	browserWarmupReply  = "Browser fixture saved."
)

type browserWarmupProvider struct{ fixtureResponsesProvider }

func (p browserWarmupProvider) respond(ctx context.Context, body []byte, headers http.Header) (*http.Response, error) {
	var request struct {
		Input []struct {
			Role    string `json:"role"`
			Content []struct {
				Text string `json:"text"`
			} `json:"content"`
		} `json:"input"`
	}
	reply := ""
	if bytes.Contains(body, []byte("Generate a concise session title")) {
		reply = "Browser fixture"
	} else if json.Unmarshal(body, &request) == nil {
		for _, item := range slices.Backward(request.Input) {
			if item.Role != "user" {
				continue
			}
			if len(item.Content) == 1 && item.Content[0].Text == browserWarmupPrompt {
				reply = browserWarmupReply
			}
			break
		}
	}
	if reply == "" {
		return p.fixtureResponsesProvider.respond(ctx, body, headers)
	}
	return nativeCollabResponse("", reply)
}

func runAppServerPreview(t *testing.T, provider fixtureResponsesProvider) {
	t.Helper()
	runAppServerPreviewWith(t, provider, nil, appServerPreview{})
}

func runAppServerPreviewWith(t *testing.T, provider fixtureResponsesProvider, _ *observationFixture, options appServerPreview) {
	t.Helper()
	binary := "codex"
	if options.managedRuntime {
		binary = "mekugi-app-server"
	}
	codex, err := exec.LookPath(binary)
	if err != nil {
		t.Fatal(err)
	}
	if options.saveBeforePrompt {
		provider = browserWarmupProvider{provider}
	}
	server := httptest.NewServer(fixtureResponsesHandler(provider))
	defer server.Close()
	environment := append(routerFaultCodexEnvironment(t), options.environment...)
	workspace := options.workspace
	if workspace == "" {
		workspace = t.TempDir()
	}
	args := []string{"app-server", "-c", `model_providers.preview={name="preview",base_url=` + strconv.Quote(server.URL+"/v1") + `,wire_api="responses",requires_openai_auth=false}`, "-c", `model_provider="preview"`, "-c", `model="gpt-6-astra"`, "-c", "features.plugins=false"}
	if options.managedRuntime {
		args = args[1:]
	}
	args = append(args, options.codexArgs...)
	var integration appServerIntegration
	if options.integration != nil {
		integration = *options.integration
	}
	termSess := startAppResumeTerminalWithIntegration(t, func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex, args...)
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}, "", integration)
	await := func(needle string) {
		t.Helper()
		termSess.await(needle)
	}
	awaitFrame := func(match func(string) bool) {
		t.Helper()
		termSess.awaitMatch("custom predicate", match)
	}
	termSess.awaitReady()
	if options.saveBeforePrompt {
		termSess.send(browserWarmupPrompt + "\r")
		termSess.await(browserWarmupReply)
	}
	if options.beforePrompt != nil {
		options.beforePrompt(t, termSess.outer, await, awaitFrame, termSess.screen)
	}
	termSess.send("Run preview fixture\r")
	if options.duringTurn != nil {
		options.duringTurn(t, termSess.outer, await, awaitFrame, termSess.screen)
	}
	completedText := options.completedText
	if completedText == "" {
		completedText = "Recovered after a retry."
	}
	termSess.await(completedText)
	if options.afterPrompt != nil {
		options.afterPrompt(t, termSess.outer, await, awaitFrame, termSess.screen)
	}
}

const routerTestWorkerEnvironment = "MEKUGI_ROUTER_TEST_WORKER"

func answerImageFixture(t *testing.T) string {
	t.Helper()
	var data bytes.Buffer
	if err := png.Encode(&data, image.NewRGBA(image.Rect(0, 0, 2, 2))); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "answer image.png")
	if err := os.WriteFile(path, data.Bytes(), 0600); err != nil {
		t.Fatal(err)
	}
	return path
}

func composerStatusShows(screen, status string) bool {
	rows := strings.Split(screen, "\n")
	for i := 0; i+1 < len(rows); i++ {
		next := i + 1
		if strings.HasPrefix(rows[next], "─") && next+1 < len(rows) {
			next++
		}
		if strings.HasPrefix(rows[i], status) && strings.HasPrefix(rows[next], "❯") {
			return true
		}
	}
	return false
}

func statLineNames(frame, thread string) bool {
	for row := range strings.Lines(frame) {
		row = strings.TrimRight(row, " \n")
		if row == "  "+thread || strings.HasPrefix(row, "  "+thread+" • ") ||
			thread != "Main" && (row == "  Main • "+thread || strings.HasPrefix(row, "  Main • "+thread+" • ")) {
			return true
		}
	}
	return false
}

func quoteShellWord(value string) string {
	return "'" + string(bytes.ReplaceAll([]byte(value), []byte("'"), []byte(`'\''`))) + "'"
}

func execTrackPath() string {
	entries := filepath.SplitList(os.Getenv("PATH"))
	entries = slices.DeleteFunc(entries, func(entry string) bool {
		base := filepath.Base(entry)
		return base == "vcs-guard" || base == "sudo-guard"
	})
	return strings.Join(entries, string(os.PathListSeparator))
}

func execTrackShellExecutable(t *testing.T, name string) string {
	t.Helper()
	for _, directory := range filepath.SplitList(execTrackPath()) {
		candidate, err := filepath.Abs(filepath.Join(directory, name))
		if err != nil {
			t.Fatal(err)
		}
		if path, err := exec.LookPath(candidate); err == nil {
			if resolved, err := filepath.EvalSymlinks(path); err == nil && filepath.Base(resolved) == "mise" {
				continue
			}
			return path
		}
	}
	t.Fatalf("missing executable %q in isolated PATH", name)
	return ""
}

func bundledModelCatalog(ctx context.Context, executable string) ([]byte, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	file, err := os.Open(filepath.Join(filepath.Dir(executable), "mekugi-models.json"))
	if err != nil {
		return nil, fmt.Errorf("open native model catalog: %w", err)
	}
	defer file.Close()
	const maxCatalogBytes = 8 << 20
	body, readErr := io.ReadAll(io.LimitReader(file, maxCatalogBytes+1))
	if readErr != nil {
		return nil, fmt.Errorf("read Codex model catalog: %w", readErr)
	}
	if len(body) > maxCatalogBytes {
		return nil, errors.New("codex model catalog exceeds 8 MiB")
	}
	return body, ctx.Err()
}

func compressResumeSettingsRollout(t *testing.T, path string) {
	t.Helper()
	zstdBin, err := exec.LookPath("zstd")
	if err != nil {
		t.Skip("zstd CLI unavailable for compressed rollout fixture")
	}
	if out, err := exec.CommandContext(t.Context(), zstdBin, "-q", "--rm", path).CombinedOutput(); err != nil {
		t.Fatalf("zstd compress %s: %v\n%s", path, err, out)
	}
}

func browserFixtureCommand(t *testing.T) string {
	t.Helper()
	name := os.Getenv("MEKUGI_BROWSER_FIXTURE")
	if name == "" {
		name = "agent-browser"
	}
	path, err := exec.LookPath(name)
	if err != nil {
		t.Fatalf("missing browser fixture command %q: %v", name, err)
	}
	return path
}

type toolFrontendCodexProvider struct {
	mu             sync.Mutex
	program        string
	expected       []string
	finalText      string
	prediction     string
	turns          int
	callSent       bool
	resultSeen     bool
	output         string
	observeRequest func([]byte, http.Header) error
}

func (p *toolFrontendCodexProvider) respond(_ context.Context, body []byte, headers http.Header) (*http.Response, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.turns++
	if p.observeRequest != nil {
		if err := p.observeRequest(body, headers); err != nil {
			return nil, err
		}
	}
	var item map[string]any
	switch {
	case p.prediction != "" && bytes.Contains(body, []byte("Predict the next message")):
		item = map[string]any{
			"type": "message", "id": "frontend-prediction", "role": "assistant", "status": "completed",
			"content": []any{map[string]any{
				"type": "output_text", "text": p.prediction, "annotations": []any{},
			}},
		}
	case !p.callSent:
		if !bytes.Contains(body, []byte("exec_command")) {
			return nil, errors.New("Codex request does not advertise stock exec_command")
		}
		p.callSent = true
		item = map[string]any{
			"type": "custom_tool_call", "id": "frontend-item", "call_id": "frontend-call",
			"name": "exec", "status": "completed", "input": p.program,
		}
	default:
		var request struct {
			Input []map[string]jsontext.Value `json:"input"`
		}
		if err := json.Unmarshal(body, &request); err != nil {
			return nil, err
		}
		for _, input := range request.Input {
			if e2eStringField(input, "type") != "custom_tool_call_output" || e2eStringField(input, "call_id") != "frontend-call" {
				continue
			}
			output := string(input["output"])
			p.output = output
			p.resultSeen = true
			for _, expected := range p.expected {
				p.resultSeen = p.resultSeen && strings.Contains(output, expected)
			}
		}
		if !p.resultSeen {
			return nil, fmt.Errorf("stock tool result lacks expected output: %.1500s", p.output)
		}
		item = map[string]any{
			"type": "message", "id": "frontend-finish", "role": "assistant", "status": "completed",
			"content": []any{map[string]any{
				"type": "output_text", "text": p.finalText, "annotations": []any{},
			}},
		}
	}
	return mchangesNestedCodexResponse(p.turns, item), nil
}

func startDirectNativeAppServerUI(t *testing.T, ctx context.Context, cmd *exec.Cmd, stdin, stdout *os.File, integration appServerIntegration, resumeThread string) (string, func() error, error) {
	t.Helper()
	socketDir, err := os.MkdirTemp("", "mkg-e2e-")
	if err != nil {
		return "", nil, err
	}
	t.Cleanup(func() { _ = os.RemoveAll(socketDir) })
	socketPath := filepath.Join(socketDir, "native.sock")
	listenArg := "unix://" + socketPath

	serverArgs := append([]string(nil), cmd.Args[1:]...)
	if len(serverArgs) > 0 && serverArgs[0] == "app-server" {
		serverArgs = append(serverArgs[:1], append([]string{"--listen", listenArg}, serverArgs[1:]...)...)
	} else {
		serverArgs = append([]string{"--listen", listenArg}, serverArgs...)
	}
	proc := exec.CommandContext(ctx, cmd.Path, serverArgs...)
	proc.Env = cmd.Env
	proc.Dir = cmd.Dir
	var stderrBuf bytes.Buffer
	proc.Stderr = &stderrBuf
	if err := proc.Start(); err != nil {
		return "", nil, err
	}
	t.Cleanup(func() {
		if proc.Process != nil {
			_ = proc.Process.Kill()
		}
		_ = proc.Wait()
	})
	deadline := time.Now().Add(15 * time.Second)
	for {
		if _, err := os.Stat(socketPath); err == nil {
			break
		}
		if time.Now().After(deadline) {
			return "", nil, fmt.Errorf("native app-server socket %s not ready: %s", socketPath, stderrBuf.String())
		}
		time.Sleep(25 * time.Millisecond)
	}
	if integration.controlSocket != nil {
		select {
		case integration.controlSocket <- socketPath:
		default:
		}
	}
	conn, err := openDirectNativeConnection(ctx, socketPath, cmd.Dir, nil, integration.launchArgs, resumeThread, "")
	if err != nil {
		return "", nil, err
	}
	views, err := composition.NativeRenderer(false)
	if err != nil {
		_ = conn.Close()
		return "", nil, err
	}
	terminal, _ := views.NewAttachment(diffrender.DarkTheme)
	client := &nativeClient{
		connection: conn,
		renderer:   nativeClientRenderer{terminal: terminal, views: views},
		editors:    make(map[string]*composer.Editor),
	}
	return socketPath, func() error {
		defer conn.Close()
		_, err := client.run(ctx, stdin, stdout)
		return err
	}, nil
}

func startAppResumeTerminalWithDirectNative(t *testing.T, newCommand func(context.Context) *exec.Cmd, resumeThread string, integration appServerIntegration) *appResumeTerminal {
	t.Helper()
	ctx, cancel := context.WithTimeout(t.Context(), 45*time.Second)
	outer, inner, err := pty.Open()
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	t.Cleanup(func() { cancel(); outer.Close(); inner.Close() })
	if err := pty.Setsize(outer, &pty.Winsize{Cols: 100, Rows: 30}); err != nil {
		t.Fatal(err)
	}
	before, err := term.GetState(int(inner.Fd()))
	if err != nil {
		t.Fatal(err)
	}
	socket, wait, err := startDirectNativeAppServerUI(t, ctx, newCommand(ctx), inner, inner, integration, resumeThread)
	if err != nil {
		t.Fatal(err)
	}
	session := observeAppResumeTerminal(t, ctx, cancel, outer, inner, before, wait)
	session.socket = socket
	return session
}
