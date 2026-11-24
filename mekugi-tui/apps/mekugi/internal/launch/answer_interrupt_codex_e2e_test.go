//go:build codex_e2e

package launch

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"

	"github.com/coder/websocket"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"

	json "encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// Hold actual input deltas, rather than holding provider admission or a
// completed tool result. The installed Codex still executes every host tool.
type answerInterruptCodexProvider struct {
	t                *testing.T
	mu               sync.Mutex
	turn             int
	mode, input      string
	socket           bool
	anonymous        bool
	streamedIdentity bool
	inputSplit       int
	gate             chan struct{}
	requests         chan []byte
}

func (p *answerInterruptCodexProvider) respond(ctx context.Context, body []byte, _ http.Header) (*http.Response, error) {
	p.mu.Lock()
	p.turn++
	turn := p.turn
	p.mu.Unlock()
	select {
	case p.requests <- bytes.Clone(body):
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	if turn == 1 && p.mode != "steer" {
		question := &questionCodexProvider{mode: "async", requests: make(chan []byte, 1)}
		return question.respond(ctx, body, nil)
	}
	if turn == 2 && p.mode == "message" || turn == 1 && p.mode == "steer" {
		reader, writer := io.Pipe()
		stop := context.AfterFunc(ctx, func() { _ = writer.CloseWithError(ctx.Err()) })
		go func() {
			defer stop()
			defer writer.Close()
			item := instantInterruptAnswer("The latest trace is complete.")
			item["id"] = "streaming-message"
			if p.anonymous {
				delete(item, "id")
			}
			item["status"] = "in_progress"
			item["content"] = []any{}
			emit := func(events ...any) bool {
				_, err := io.WriteString(writer, fixtureResponsesSSE(events...))
				return err == nil
			}
			added := map[string]any{"type": "response.output_item.added", "output_index": 0, "item": item}
			delta := map[string]any{"type": "response.output_text.delta", "item_id": item["id"], "output_index": 0, "content_index": 0, "delta": "The latest trace"}
			if p.streamedIdentity {
				if p.mode == "message" {
					delete(item, "id") // The delta supplies the ID before ID-only completion.
				} else {
					delete(added, "output_index") // The delta supplies the index instead.
				}
			}
			if !emit(
				map[string]any{"type": "response.created", "response": map[string]any{"id": "message-emission", "status": "in_progress", "output": []any{}}},
				added, delta,
			) {
				return
			}
			select {
			case <-p.gate:
			case <-ctx.Done():
				return
			}
			item = instantInterruptAnswer("The latest trace is complete.")
			item["id"] = "streaming-message"
			if p.anonymous {
				delete(item, "id")
			}
			done := map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item}
			if p.streamedIdentity {
				if p.mode == "message" {
					delete(done, "output_index")
				} else {
					delete(item, "id")
				}
			}
			if !emit(
				map[string]any{"type": "response.output_text.delta", "item_id": item["id"], "output_index": 0, "content_index": 0, "delta": " is complete."},
				map[string]any{"type": "response.output_text.done", "item_id": item["id"], "output_index": 0, "content_index": 0, "text": "The latest trace is complete."},
				done,
			) {
				return
			}
			// Consecutive HTTP messages test that accepted input retains the first
			// message's boundary even when the next message starts in the same read.
			if p.streamedIdentity {
				if !emit(map[string]any{"type": "response.output_item.added", "output_index": 1, "item": map[string]any{"id": "reasoning", "type": "reasoning", "summary": []any{}}}) {
					return
				}
			} else if !p.socket {
				if !emit(
					map[string]any{"type": "response.output_item.added", "output_index": 1, "item": map[string]any{"id": "next-message", "type": "message", "role": "assistant", "status": "in_progress", "content": []any{}}},
					map[string]any{"type": "response.output_text.delta", "output_index": 1, "content_index": 0, "item_id": "next-message", "delta": "Another message is streaming"},
				) {
					return
				}
			}
			// Neither transport completes its response or native turn at this gate.
			<-ctx.Done()
		}()
		return &http.Response{StatusCode: http.StatusOK, Header: http.Header{"Content-Type": {"text/event-stream"}}, Body: reader}, nil
	}
	if turn == 2 && p.mode == "steer" {
		return mchangesNestedCodexResponse(turn, instantInterruptAnswer("Answer emission acceptance complete")), nil
	}
	if turn == 2 {
		reader, writer := io.Pipe()
		stop := context.AfterFunc(ctx, func() { _ = writer.CloseWithError(ctx.Err()) })
		go func() {
			defer stop()
			defer writer.Close()
			item := map[string]any{"type": "custom_tool_call", "id": "answer-tool-item", "call_id": "answer-tool", "name": "exec", "status": "in_progress", "input": ""}
			emit := func(events ...any) bool {
				_, err := io.WriteString(writer, fixtureResponsesSSE(events...))
				return err == nil
			}
			split := p.inputSplit
			if split == 0 {
				split = len(p.input) / 2
			}
			if !emit(
				map[string]any{"type": "response.created", "response": map[string]any{"id": "answer-emission", "status": "in_progress", "output": []any{}}},
				map[string]any{"type": "response.output_item.added", "output_index": 0, "item": item},
				map[string]any{"type": "response.custom_tool_call_input.delta", "item_id": item["id"], "output_index": 0, "delta": p.input[:split]},
			) {
				return
			}
			select {
			case <-p.gate:
			case <-ctx.Done():
				return
			}
			if !emit(map[string]any{"type": "response.custom_tool_call_input.delta", "item_id": item["id"], "output_index": 0, "delta": p.input[split:]}) {
				return
			}
			item["input"], item["status"] = p.input, "completed"
			emit(
				map[string]any{"type": "response.custom_tool_call_input.done", "item_id": item["id"], "output_index": 0, "input": p.input},
				map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item},
				map[string]any{"type": "response.completed", "response": map[string]any{"id": "answer-emission", "status": "completed", "output": []any{item}, "usage": map[string]any{"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}},
			)
		}()
		return &http.Response{StatusCode: http.StatusOK, Header: http.Header{"Content-Type": {"text/event-stream"}}, Body: reader}, nil
	}
	if turn == 3 && p.mode == "exec" {
		var request struct {
			Input []struct {
				Type   string         `json:"type"`
				CallID string         `json:"call_id"`
				Output jsontext.Value `json:"output"`
			} `json:"input"`
		}
		if err := json.Unmarshal(body, &request); err != nil {
			return nil, err
		}
		for _, item := range request.Input {
			if item.Type == "custom_tool_call_output" && item.CallID == "answer-tool" {
				_, cell, _ := binding.CodeModeExecutionHeader(strings.Join(binding.ExecutionOutputTexts(item.Output), "\n"))
				if cell != "" {
					return mchangesNestedCodexResponse(turn, map[string]any{"type": "function_call", "id": "answer-wait-item", "call_id": "answer-wait", "name": "wait", "status": "completed", "arguments": string(responses.MustMarshalJSON(map[string]any{"cell_id": cell, "yield_time_ms": 10000}))}), nil
				}
			}
		}
		return nil, fmt.Errorf("answer did not yield the running exec cell: %.2000s", body)
	}
	if turn == 4 && p.mode == "exec" {
		var request struct {
			Input []struct {
				Type   string         `json:"type"`
				CallID string         `json:"call_id"`
				Output jsontext.Value `json:"output"`
			} `json:"input"`
		}
		if err := json.Unmarshal(body, &request); err != nil {
			return nil, err
		}
		completed := false
		for _, item := range request.Input {
			if item.Type == "function_call_output" && item.CallID == "answer-wait" {
				output := strings.Join(binding.ExecutionOutputTexts([]byte(item.Output)), "\n")
				completed = strings.Contains(output, "input-survived") && strings.Contains(output, "Script completed")
			}
		}
		if !completed {
			return nil, fmt.Errorf("wait did not complete the original exec cell")
		}
	}
	if turn == 3 || turn == 4 && p.mode == "exec" {
		return mchangesNestedCodexResponse(turn, instantInterruptAnswer("Answer emission acceptance complete")), nil
	}
	return nil, fmt.Errorf("unexpected provider request %d", turn)
}

type answerInterruptResponseWriter struct {
	http.ResponseWriter
	emitted chan struct{}
}

func (w *answerInterruptResponseWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }
func (w *answerInterruptResponseWriter) Write(body []byte) (int, error) {
	n, err := w.ResponseWriter.Write(body)
	if err == nil && (bytes.Contains(body[:n], []byte(`"type":"response.custom_tool_call_input.delta"`)) || bytes.Contains(body[:n], []byte(`"type":"response.output_text.delta"`))) {
		select {
		case w.emitted <- struct{}{}:
		default:
		}
	}
	return n, err
}

func TestAnswerInterruptNativeCodexE2E(t *testing.T) {
	for _, tc := range []struct {
		mode             string
		socket           bool
		anonymous        bool
		streamedIdentity bool
	}{{"apply_patch", false, false, false}, {"exec", false, false, false}, {"message", false, false, false}, {"steer", false, false, false}, {"message", true, false, false}, {"steer", true, false, false}, {"message", false, true, false}, {"steer", true, true, false}, {"message", false, false, true}, {"steer", false, false, true}, {"message", true, false, true}, {"steer", true, false, true}} {
		mode := tc.mode
		t.Run(fmt.Sprintf("%s/socket=%v/anonymous=%v/streamedIdentity=%v", mode, tc.socket, tc.anonymous, tc.streamedIdentity), func(t *testing.T) {
			binary, subcommand := "codex", []string{"app-server"}
			if mode == "apply_patch" || mode == "exec" {
				binary, subcommand = "mekugi-app-server", nil
			}
			codex, err := exec.LookPath(binary)
			if err != nil {
				t.Fatalf("%s is required for answer interruption acceptance: %v", binary, err)
			}
			t.Setenv("XDG_STATE_HOME", t.TempDir())
			workspace := t.TempDir()
			gatePath := filepath.Join(workspace, "release")
			startedPath := filepath.Join(workspace, "started")
			countPath := filepath.Join(workspace, "count")
			patchPath := filepath.Join(workspace, "answer-patch.txt")
			provider := &answerInterruptCodexProvider{t: t, mode: mode, socket: tc.socket, anonymous: tc.anonymous, streamedIdentity: tc.streamedIdentity, gate: make(chan struct{}), requests: make(chan []byte, 8)}
			if mode == "apply_patch" {
				patch := "*** Begin Patch\n*** Add File: answer-patch.txt\n+input-survived\n+after-release\n*** End Patch"
				provider.input = "await tools.apply_patch(" + string(responses.MustMarshalJSON(patch)) + ");"
				provider.inputSplit = strings.Index(provider.input, "+after-release")
			} else {
				shell := "echo run >> " + strconv.Quote(countPath) + "; touch " + strconv.Quote(startedPath) + "; while test ! -f " + strconv.Quote(gatePath) + "; do sleep 0.1; done; echo input-survived"
				provider.input = "// @exec: {\"yield_time_ms\": 60000}\nconst result = await tools.exec_command({cmd: " + string(responses.MustMarshalJSON(shell)) + ", workdir: " + string(responses.MustMarshalJSON(workspace)) + ", yield_time_ms: 30000}); text(result.output);"
			}
			proxy := newManagedMekugiProxy(t)
			attachTestReplayStore(t, proxy)
			if mode == "apply_patch" {
				preview := newLivePreview(t.Context())
				proxy.review = testReview(preview)
				t.Cleanup(preview.Close)
			}
			emitted := make(chan struct{}, 2)
			handler := fixtureResponsesHandler(provider)
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
					provider.serveSocket(w, r)
					return
				}
				handler(&answerInterruptResponseWriter{ResponseWriter: w, emitted: emitted}, r)
			}))
			defer server.Close()
			gateClosed := false
			defer func() {
				if !gateClosed {
					close(provider.gate)
				}
				_ = os.WriteFile(gatePath, nil, 0o600)
			}()
			environment := routerFaultCodexEnvironment(t)
			terminal := startAppResumeTerminalWithIntegration(t, func(ctx context.Context) *exec.Cmd {
				args := append(subcommand, "-c", "features.instant_interrupt=true", "-c", `model_providers.answer_fixture={name="OpenAI",base_url=`+strconv.Quote(server.URL+"/v1")+`,wire_api="responses",requires_openai_auth=false,supports_websockets=`+strconv.FormatBool(tc.socket)+`}`, "-c", `model_provider="answer_fixture"`, "-c", `model="gpt-6-astra"`, "-c", "features.plugins=false", "-c", "features.code_mode=true", "-c", "features.code_mode_host=true", "-c", "features.default_mode_request_user_input=true")
				cmd := exec.CommandContext(ctx, codex, args...)
				cmd.Env, cmd.Dir = environment, workspace
				return cmd
			}, "", testAppServerIntegration(proxy))
			defer terminal.cancel()
			next := func() []byte {
				t.Helper()
				select {
				case request := <-provider.requests:
					return request
				case <-terminal.ctx.Done():
					t.Fatalf("provider request missing:\n%s", terminal.screen.String())
					return nil
				}
			}
			terminal.await("Ready")
			terminal.send("Ask the release question, then perform held work\r")
			_ = next()
			if mode != "steer" {
				terminal.await("Who should receive the update?")
				_ = next()
			}
			// This signal comes from the consumer-facing HTTP writer, after both
			// response.created and output_item.added, not from provider admission.
			if tc.socket {
				terminal.await("The latest trace")
			} else {
				select {
				case <-emitted:
				case <-terminal.ctx.Done():
					t.Fatal("input delta was not delivered")
				}
			}
			var toolStatus string
			switch mode {
			case "apply_patch":
				toolStatus = "Emitting a patch"
			case "exec":
				toolStatus = "Executing a command"
			}
			if toolStatus != "" {
				// The question overlay replaces the composer activity label.
				terminal.send("\x1b")
				terminal.await(toolStatus)
			}
			if mode == "steer" {
				terminal.send("Change direction now\r")
			} else {
				terminal.send("\x02q")
				terminal.await("1 of 1")
				terminal.send("1\r")
			}
			terminal.awaitMatch("accepted queued answer", func(screen string) bool {
				working := strings.Contains(screen, "Working") || toolStatus != "" && strings.Contains(screen, toolStatus)
				queued := strings.Contains(screen, "Steering pending") && working && !strings.Contains(screen, "Sending…")
				// Let a broken early-send path reach the request assertion below.
				answered := strings.Contains(screen, "Who should receive the update?") && strings.Contains(screen, "Customers") && !strings.Contains(screen, "1 of 1")
				return queued || answered || mode == "steer" && strings.Contains(screen, "Change direction now")
			})
			if mode == "apply_patch" {
				// The question dock has priority until the answer is queued.
				terminal.awaitMatch("live patch source", func(screen string) bool {
					return strings.Contains(screen, "LIVE") && strings.Contains(screen, "input-survived")
				})
				if _, err := os.Stat(patchPath); !os.IsNotExist(err) {
					t.Fatalf("preview did not precede tool execution: %v", err)
				}
			}
			select {
			case <-provider.requests:
				t.Fatalf("answer interrupted unfinished %s input", mode)
			case <-time.After(300 * time.Millisecond):
			case <-terminal.ctx.Done():
				t.Fatal(terminal.ctx.Err())
			}
			close(provider.gate)
			gateClosed = true
			request := next()
			var parsed struct {
				Input []any `json:"input"`
			}
			if err := json.Unmarshal(request, &parsed); err != nil {
				t.Fatal(err)
			}
			input, err := json.Marshal(parsed.Input)
			if err != nil {
				t.Fatal(err)
			}
			if mode == "steer" {
				if bytes.Count(input, []byte("Change direction now")) != 1 {
					t.Fatalf("steer was not delivered exactly once: %.4000s", input)
				}
			} else {
				// Native developer guidance also mentions the answer delimiter.
				var answers []binding.QuestionAnswer
				for _, value := range parsed.Input {
					item, ok := value.(map[string]any)
					if !ok || item["role"] != "user" {
						continue
					}
					content, _ := item["content"].([]any)
					for _, value := range content {
						block, _ := value.(map[string]any)
						text, _ := block["text"].(string)
						answers = append(answers, binding.QuestionReplies(text)...)
					}
				}
				if len(answers) != 1 || answers[0].ID != "q1" || answers[0].Choice != 1 {
					t.Fatalf("answer was not delivered exactly once after input completed: %+v", answers)
				}
			}
			if mode == "message" || mode == "steer" {
				if !tc.socket && !bytes.Contains(input, []byte("The latest trace is complete.")) {
					t.Fatalf("steering truncated the assistant message: %.2000s", input)
				}
			}
			if mode == "exec" {
				if _, err := os.Stat(gatePath); !os.IsNotExist(err) {
					t.Fatalf("answer waited for command gate: %v", err)
				}
				if err := os.WriteFile(gatePath, nil, 0o600); err != nil {
					t.Fatal(err)
				}
				_ = next()
				if _, err := os.Stat(startedPath); err != nil {
					t.Fatalf("host command did not start: %v", err)
				}
				count, err := os.ReadFile(countPath)
				if err != nil || string(count) != "run\n" {
					t.Fatalf("host command execution count %q, %v", count, err)
				}
			} else if mode == "apply_patch" {
				data, err := os.ReadFile(patchPath)
				if err != nil || string(data) != "input-survived\nafter-release\n" {
					t.Fatalf("streamed patch did not execute intact: %q, %v", data, err)
				}
			}
			if mode == "message" || mode == "steer" {
				terminal.await("The latest trace is complete.")
			}
			terminal.await("Answer emission acceptance complete")
			terminal.await("completed")
			terminal.quit()
		})
	}
}

// Serve the same deterministic stream through the real provider WebSocket path.
func (p *answerInterruptCodexProvider) serveSocket(w http.ResponseWriter, r *http.Request) {
	conn, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	defer conn.CloseNow()
	cancelResponse := func() {}
	defer func() { cancelResponse() }()
	for {
		fields, err := readFixtureResponseSocket(r.Context(), conn)
		if err != nil {
			return
		}
		if responses.StringField(fields, "type") == "response.interrupt" {
			cancelResponse()
			item := instantInterruptAnswer("The latest trace is complete.")
			item["id"] = "streaming-message"
			if writeFixtureResponseSocket(r.Context(), conn, map[string]any{"type": "response.interrupt.accepted", "response_id": "message-emission"}) != nil ||
				writeFixtureResponseSocket(r.Context(), conn, map[string]any{"type": "response.incomplete", "response": map[string]any{"id": "message-emission", "status": "incomplete", "incomplete_details": map[string]string{"reason": "interrupted"}, "output": []any{item}}}) != nil {
				return
			}
			continue
		}
		if string(fields["generate"]) == "false" {
			if writeFixtureResponseSocket(r.Context(), conn, fixtureSocketEvent("response.created", "warm")) != nil || writeFixtureResponseSocket(r.Context(), conn, fixtureSocketEvent("response.completed", "warm")) != nil {
				return
			}
			continue
		}
		body, err := json.Marshal(fields)
		if err != nil {
			return
		}
		cancelResponse()
		responseCtx, cancel := context.WithCancel(r.Context())
		cancelResponse = cancel
		response, err := p.respond(responseCtx, body, r.Header)
		if err != nil {
			return
		}
		// Keep reading so cancellation/connection closure retires an unfinished
		// response instead of blocking the fixture in its held response body.
		go func() {
			defer cancel()
			defer response.Body.Close()
			scanner := bufio.NewScanner(response.Body)
			for scanner.Scan() {
				if payload, ok := bytes.CutPrefix(scanner.Bytes(), []byte("data: ")); ok {
					if err := conn.Write(r.Context(), websocket.MessageText, payload); err != nil {
						conn.CloseNow()
						return
					}
				}
			}
			if scanner.Err() != nil && responseCtx.Err() == nil {
				conn.CloseNow()
			}
		}()
	}
}
