//go:build codex_e2e

package launch

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"unicode/utf8"

	json "encoding/json/v2"

	uv "github.com/charmbracelet/ultraviolet"
)

// Hold only the fork's final provider round so Main can finish independently
// after the native tool has executed and returned its result to the model.
type oneoffCodexProvider struct {
	tools    toolFrontendCodexProvider
	requests chan btwCodexRequest
	finish   chan struct{}
}

func (p *oneoffCodexProvider) respond(ctx context.Context, body []byte, headers http.Header) (*http.Response, error) {
	p.requests <- btwCodexRequest{bytes.Clone(body), fixturePromptCacheKey(body), headers.Clone()}
	if bytes.Contains(body, []byte("ONEOFF_TASK_ONLY")) {
		response, err := p.tools.respond(ctx, body, headers)
		if err != nil {
			return nil, err
		}
		p.tools.mu.Lock()
		resultSeen := p.tools.resultSeen
		p.tools.mu.Unlock()
		if resultSeen {
			select {
			case <-p.finish:
			case <-ctx.Done():
				response.Body.Close()
				return nil, ctx.Err()
			}
		}
		return response, nil
	}
	response := fixtureResponsesSuccess()
	wire, err := io.ReadAll(response.Body)
	response.Body.Close()
	if err != nil {
		return nil, err
	}
	answer := "MAIN_SEED_ANSWER"
	if bytes.Contains(body, []byte("BTW_WHILE_ONEOFF")) {
		answer = "BTW_INDEPENDENT_ANSWER"
	}
	if bytes.Contains(body, []byte("Main continuation")) {
		answer = "MAIN_CONTINUATION_FINISHED"
	}
	response.Body = io.NopCloser(bytes.NewReader(bytes.ReplaceAll(wire, []byte("Recovered after a retry."), []byte(answer))))
	return response, nil
}

func TestAppServerOneoffNativeCodexE2E(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	codex, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	workspace := t.TempDir()
	marker := filepath.Join(workspace, "oneoff-written.txt")
	p := &oneoffCodexProvider{
		requests: make(chan btwCodexRequest, 12), finish: make(chan struct{}),
		tools: toolFrontendCodexProvider{
			program:  `const result = await tools.exec_command({cmd:"printf '%s' \"$MEKUGI_ONEOFF_TEST\" > oneoff-written.txt; cat oneoff-written.txt",login:false}); text(JSON.stringify({output:result.output,exit_code:result.exit_code}));`,
			expected: []string{"ONEOFF_ENV_INHERITED", `\"exit_code\":0`}, finalText: "ONEOFF_FINISHED",
		},
	}
	proxy := newManagedMekugiProxy(t)
	server := httptest.NewServer(fixtureResponsesHandler(p))
	defer server.Close()
	defer func() {
		select {
		case <-p.finish:
		default:
			close(p.finish)
		}
	}()
	environment := append(routerFaultCodexEnvironment(t), "MEKUGI_ONEOFF_TEST=ONEOFF_ENV_INHERITED")
	command := func(ctx context.Context) *exec.Cmd {
		cmd := exec.CommandContext(ctx, codex,
			"-c", `model_providers.preview={name="OpenAI",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false}`,
			"-c", `model_provider="preview"`, "-c", `model="gpt-6-astra"`,
			"-c", "features.plugins=false",
			"-c", `sandbox_mode="danger-full-access"`, "-c", `approval_policy="never"`)
		cmd.Env, cmd.Dir = environment, workspace
		return cmd
	}
	terminal := startAppResumeTerminalWithIntegration(t, command, "", testAppServerIntegration(proxy))
	next := func() btwCodexRequest {
		t.Helper()
		select {
		case request := <-p.requests:
			return request
		case <-terminal.ctx.Done():
			t.Fatalf("missing provider request: %v\n%s", terminal.ctx.Err(), terminal.screen.String())
		}
		return btwCodexRequest{}
	}
	selected := func(label string, column int) bool {
		y := terminal.screen.Height() - 1
		for i, ch := range label {
			cell := terminal.screen.CellAt(column+i, y)
			if cell == nil || cell.Content != string(ch) || cell.Style.Attrs&uv.AttrBold == 0 {
				return false
			}
		}
		return true
	}
	terminal.awaitReady()
	terminal.send("Main seed\r")
	main := next()
	mainMetadata, _ := fixtureTurnMetadata(main.headers)
	terminal.await("MAIN_SEED_ANSWER")
	terminal.awaitThreadIdle(mainMetadata.ThreadID)
	terminal.send("/oneoff ONEOFF_TASK_ONLY\r")
	fork := next()
	forkMetadata, _ := fixtureTurnMetadata(fork.headers)
	if mainMetadata.ThreadID == "" || forkMetadata.ThreadID == "" || mainMetadata.ThreadID == forkMetadata.ThreadID {
		t.Fatalf("fork identities: Main=%q oneoff=%q", mainMetadata.ThreadID, forkMetadata.ThreadID)
	}
	if !bytes.Contains(fork.body, []byte("Main seed")) || btwLastUserPrompt(fork.body) != "ONEOFF_TASK_ONLY" {
		t.Fatal("one-off omitted inherited history or altered the task")
	}
	var mainFields, forkFields map[string]any
	if err := json.Unmarshal(main.body, &mainFields); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(fork.body, &forkFields); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(mainFields["tools"], forkFields["tools"]) {
		t.Fatal("one-off did not inherit Main's tools")
	}
	terminal.awaitMatch("one-off automatically selected", func(frame string) bool {
		return statLineNames(frame, "One-off") && strings.Contains(frame, "❯ ONEOFF_TASK_ONLY")
	})
	result := next()
	resultMetadata, _ := fixtureTurnMetadata(result.headers)
	if resultMetadata.ThreadID != forkMetadata.ThreadID || !bytes.Contains(result.body, []byte("custom_tool_call_output")) || !bytes.Contains(result.body, []byte("ONEOFF_ENV_INHERITED")) {
		t.Fatalf("native tool result did not return within the fork: %s", result.body)
	}
	contents, err := os.ReadFile(marker)
	if err != nil || string(contents) != "ONEOFF_ENV_INHERITED" {
		t.Fatalf("one-off write/environment inheritance: contents=%q err=%v", contents, err)
	}
	terminal.send("/btw BTW_WHILE_ONEOFF\r")
	question := next()
	questionMetadata, _ := fixtureTurnMetadata(question.headers)
	if questionMetadata.ThreadID == "" || questionMetadata.ThreadID == forkMetadata.ThreadID || questionMetadata.ThreadID == mainMetadata.ThreadID ||
		btwLastUserPrompt(question.body) != "BTW_WHILE_ONEOFF" || bytes.Contains(question.body, []byte("ONEOFF_TASK_ONLY")) {
		t.Fatalf("BTW identities: Main=%q One-off=%q BTW=%q; user input=%q, contains one-off task=%t",
			mainMetadata.ThreadID, forkMetadata.ThreadID, questionMetadata.ThreadID, btwLastUserPrompt(question.body),
			bytes.Contains(question.body, []byte("ONEOFF_TASK_ONLY")))
	}
	terminal.await("BTW_INDEPENDENT_ANSWER")
	terminal.await("/btw · completed")
	terminal.send("\x1b")
	terminal.awaitMatch("BTW closed while One-off remains", func(frame string) bool {
		return !strings.Contains(frame, "/btw ·") && !strings.Contains(frame, "BTW_INDEPENDENT_ANSWER") && selected("One-off", 9)
	})
	terminal.send("\x1b[<0;4;30M\x1b[<0;4;30m")
	terminal.awaitMatch("Main selected before continuing", func(frame string) bool {
		return selected("Main", 2) && strings.Contains(frame, "MAIN_SEED_ANSWER") &&
			!strings.Contains(frame, "ONEOFF_TASK_ONLY")
	})
	terminal.send("Main continuation\r")
	after := next()
	if prompt := btwLastUserPrompt(after.body); prompt != "Main continuation" {
		t.Fatalf("main continuation prompt = %q", prompt)
	}
	afterMetadata, _ := fixtureTurnMetadata(after.headers)
	if afterMetadata.ThreadID != mainMetadata.ThreadID || after.key != main.key {
		t.Fatal("Main continuation changed thread/cache identity")
	}
	for _, isolated := range []string{"ONEOFF_TASK_ONLY", "ONEOFF_ENV_INHERITED", "oneoff-written.txt", "frontend-call"} {
		if bytes.Contains(after.body, []byte(isolated)) {
			t.Fatalf("one-off injected %q into Main's continuation", isolated)
		}
	}
	terminal.await("MAIN_CONTINUATION_FINISHED")
	terminal.awaitThreadIdle(mainMetadata.ThreadID)
	close(p.finish)
	terminal.send("\x1b[<0;12;30M\x1b[<0;12;30m")
	terminal.awaitMatch("One-off selected and finished", func(frame string) bool { return selected("One-off", 9) && strings.Contains(frame, "ONEOFF_FINISHED") })
	terminal.awaitThreadIdle(forkMetadata.ThreadID)
	expandHostActivity(t, terminal.outer)
	terminal.awaitMatch("completed one-off retained", func(frame string) bool {
		return statLineNames(frame, "One-off") &&
			strings.Contains(frame, "printf") && strings.Contains(frame, "cat oneoff-written.txt") &&
			strings.Contains(frame, "combined output") && strings.Contains(frame, "Create oneoff-written.txt")
	})
	// The transcript shows captured changes instead of the tail for this
	// command. The combined Output view still opens its retained stdout.
	for y, row := range strings.Split(terminal.screen.String(), "\n") {
		if before, _, found := strings.Cut(row, "combined output"); found {
			x := utf8.RuneCountInString(before) + 1
			terminal.send(fmt.Sprintf("\x1b[<0;%d;%dM\x1b[<0;%d;%dm", x, y+1, x, y+1))
			break
		}
	}
	terminal.await("ONEOFF_ENV_INHERITED")
	terminal.send("\x1b")
	terminal.awaitMatch("combined Output closed", func(frame string) bool {
		return !strings.Contains(frame, "ONEOFF_ENV_INHERITED")
	})
	terminal.send("\x1b[<0;4;30M\x1b[<0;4;30m")
	terminal.awaitMatch("Main selected at end", func(frame string) bool {
		return selected("Main", 2) && strings.Contains(frame, "MAIN_CONTINUATION_FINISHED")
	})
	terminal.quit()
}
