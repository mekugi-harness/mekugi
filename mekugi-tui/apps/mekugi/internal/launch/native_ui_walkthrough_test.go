package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"maps"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

func TestNativeUIWalkthrough(t *testing.T) {
	binary := os.Getenv("MEKUGI_PREVIEW_BINARY")
	if binary == "" {
		t.Skip("use just preview-native-ui")
	}
	tty, err := os.OpenFile("/dev/tty", os.O_RDWR, 0)
	if err != nil {
		t.Fatalf("run just preview-native-ui in an interactive terminal: %v", err)
	}
	defer tty.Close()

	native, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal("native runtime not found; run just build-native first")
	}
	root := shortTempDir(t)
	workspace, codexDir := filepath.Join(root, "project"), filepath.Join(root, "codex")
	tempDir := filepath.Join(root, "tmp")
	for _, dir := range []string{workspace, codexDir, tempDir} {
		if err := os.Mkdir(dir, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("CODEX_HOME", codexDir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(root, "data"))
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(root, "cache"))
	t.Setenv("TMPDIR", tempDir)
	t.Setenv("SESSION_TMP", "")
	t.Setenv("MEKUGI_CHANGE_HISTORY_DIR", "")
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	git := exec.Command("git", "init", "--quiet", "--template=")
	git.Dir = workspace
	if output, err := git.CombinedOutput(); err != nil {
		t.Fatalf("initialize preview workspace: %v\n%s", err, output)
	}
	if err := os.WriteFile(filepath.Join(workspace, "pane.txt"), []byte("Wait for the first frame before showing the pane.\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	routes := http.NewServeMux()
	routes.HandleFunc("POST /v1/responses", new(walkthroughProvider).serve)
	provider := httptest.NewServer(routes)
	defer provider.Close()
	config := `model = "gpt-6-luna"
model_provider = "preview"
approval_policy = "never"
sandbox_mode = "workspace-write"
web_search = "disabled"
[features]
plugins = false
default_mode_request_user_input = true
multi_agent_v2 = { enabled = true, disable_native_tools = false }
[model_providers.preview]
name = "Local preview"
base_url = ` + strconv.Quote(provider.URL+"/v1") + `
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
`
	if err := os.WriteFile(filepath.Join(codexDir, "config.toml"), []byte(config), 0o600); err != nil {
		t.Fatal(err)
	}

	socket, err := nativeAppServerSocketPath()
	if err != nil {
		t.Fatal(err)
	}
	ctx, stop := signal.NotifyContext(t.Context(), os.Interrupt, syscall.SIGTERM, syscall.SIGHUP)
	defer stop()
	server := exec.CommandContext(ctx, native, "--listen", "unix://"+socket)
	server.Dir, server.Stderr = workspace, tty
	server.Cancel = func() error { return server.Process.Signal(syscall.SIGTERM) }
	server.WaitDelay = 5 * time.Second
	defer func() {
		if server.Process != nil {
			stop()
			if err := server.Wait(); err != nil && !errors.Is(err, context.Canceled) {
				t.Errorf("preview native server shutdown: %v", err)
			}
		}
	}()
	ready, cancelReady := context.WithTimeout(ctx, 10*time.Second)
	err = ensureNativeAppServer(ready, socket, server.Start)
	cancelReady()
	if err != nil {
		t.Fatal(err)
	}

	ui := exec.CommandContext(ctx, binary, "codex")
	ui.Dir, ui.Stdin, ui.Stdout, ui.Stderr = workspace, tty, tty, tty
	if err := ui.Run(); err != nil {
		t.Fatal(err)
	}
}

type walkthroughProvider struct {
	next atomic.Uint64
}

type walkthroughInput struct {
	Type    string         `json:"type"`
	Role    string         `json:"role"`
	CallID  string         `json:"call_id"`
	Output  jsontext.Value `json:"output"`
	Content []struct {
		Text string `json:"text"`
	} `json:"content"`
}

const walkthroughReview = "Review the pane.txt change."
const walkthroughPatch = `*** Begin Patch
*** Update File: pane.txt
@@
-Wait for the first frame before showing the pane.
+Show the pane while its first frame loads.
*** End Patch
`

// Choose the next step from the actual history, so retries and separate
// threads can interleave without advancing the wrong scenario.
func walkthroughStep(input []walkthroughInput) string {
	for i := len(input) - 1; i >= 0; i-- {
		item := input[i]
		switch item.Type {
		case "custom_tool_call_output", "function_call_output":
			if _, step, ok := strings.Cut(item.CallID, "-step-"); ok {
				if step == "sync-question" {
					var output string
					var answers []binding.QuestionAnswer
					if json.Unmarshal(item.Output, &output) != nil ||
						json.Unmarshal([]byte(output), &answers) != nil || len(answers) == 0 {
						return "invalid-answer"
					}
				}
				return step
			}
		case "agent_message":
			for _, part := range item.Content {
				if strings.HasSuffix(strings.TrimSpace(part.Text), "\nPayload:\n"+walkthroughReview) {
					return "review"
				}
			}
		case "message":
			if item.Role != "user" {
				continue
			}
			for _, part := range item.Content {
				prompt := strings.TrimSpace(part.Text)
				for _, reply := range parseNativeQuestionReplies(prompt) {
					if reply.Question == "Who should receive the release update?" &&
						(reply.Answer == "Customers" || reply.Answer == "Internal team") {
						return "audience-reply"
					}
				}
				if prompt != "" && !strings.HasPrefix(prompt, "<") {
					return "user:" + strings.ToLower(prompt)
				}
			}
		}
	}
	return ""
}

func (p *walkthroughProvider) serve(w http.ResponseWriter, req *http.Request) {
	var input struct {
		Items []walkthroughInput `json:"input"`
	}
	if err := json.UnmarshalRead(req.Body, &input); err != nil {
		http.Error(w, "invalid preview request", http.StatusBadRequest)
		return
	}
	sequence := strconv.FormatUint(p.next.Add(1), 10)
	id := "preview-" + sequence
	item := map[string]any{"id": id + "-item", "status": "completed"}
	message := func(text string) {
		item["type"], item["role"], item["phase"] = "message", "assistant", "final_answer"
		item["content"] = []any{map[string]any{"type": "output_text", "text": text}}
	}
	execute := func(step, program string) {
		item["type"], item["name"], item["namespace"] = "custom_tool_call", "exec", "functions"
		item["call_id"], item["input"] = id+"-step-"+step, program
	}
	question := func(step, name, args string) {
		item["type"], item["name"] = "function_call", name
		item["call_id"], item["arguments"] = id+"-step-"+step, args
	}
	step := walkthroughStep(input.Items)
	switch step {
	case "user:demo":
		execute("edited", "text(await tools.apply_patch(`"+walkthroughPatch+"`));")
	case "edited":
		execute("delegated", `text(await tools.collaboration__spawn_agent({task_name:"reviewer_`+sequence+`",message:`+
			strconv.Quote(walkthroughReview)+`,fork_turns:"none"})); text(await tools.collaboration__wait_agent({wait_mode:"all"}));`)
	case "review":
		execute("reviewed", `text(await tools.exec_command({cmd:"cat pane.txt"}));`)
	case "reviewed":
		message("Reviewed pane.txt: the first frame no longer blocks the pane.")
	case "delegated":
		message("The pane now opens while its first frame loads. Check pane.txt in Diff.")
	case "user:questions":
		question("async-question", "request_user_input_async", `{"questions":[
			{"header":"Audience","question":"Who should receive the release update?","options":[
				{"label":"Customers","preview":"Announce the release to all customers."},
				{"label":"Internal team","preview":"Share with the team for feedback first.","recommended":true}]}]}`)
	case "async-question":
		message("Release audience question sent.")
	case "audience-reply":
		question("sync-question", "request_user_input", `{"questions":[
			{"header":"Scope","question":"Which release scope?","options":[
				{"label":"Narrow","preview":"Validate and publish only the affected path.","recommended":true},
				{"label":"Broad","preview":"Validate and publish the complete release."}]}]}`)
	case "sync-question":
		message("Release scope received.")
	case "invalid-answer":
		message("The question answer was missing or invalid. Submit questions to try again.")
	default:
		message("This is a scripted preview. Submit demo or questions.")
	}

	w.Header().Set("Content-Type", "text/event-stream")
	stream := http.NewResponseController(w)
	emit := func(kind string, event map[string]any) bool {
		event["type"] = kind
		data, err := json.Marshal(event)
		if err != nil {
			return false
		}
		_, err = fmt.Fprintf(w, "data: %s\n\n", data)
		return err == nil && stream.Flush() == nil
	}
	if !emit("response.created", map[string]any{"response": map[string]any{
		"id": id, "status": "in_progress", "output": []any{},
	}}) {
		return
	}
	initial := maps.Clone(item)
	initial["status"] = "in_progress"
	if step == "user:demo" {
		initial["input"] = ""
	}
	if !emit("response.output_item.added", map[string]any{"output_index": 0, "item": initial}) {
		return
	}
	if step == "user:demo" {
		program := item["input"].(string)
		for start := 0; start < len(program); start += 24 {
			// Pace only the edit to make the provisional diff visible.
			select {
			case <-req.Context().Done():
				return
			case <-time.After(200 * time.Millisecond):
			}
			if !emit("response.custom_tool_call_input.delta", map[string]any{
				"output_index": 0, "item_id": item["id"], "delta": program[start:min(start+24, len(program))],
			}) {
				return
			}
		}
	}
	if !emit("response.output_item.done", map[string]any{"output_index": 0, "item": item}) {
		return
	}
	emit("response.completed", map[string]any{"response": map[string]any{
		"id": id, "status": "completed", "output": []any{item},
		"usage": map[string]any{"input_tokens": 100, "output_tokens": 50, "total_tokens": 150},
	}})
}
