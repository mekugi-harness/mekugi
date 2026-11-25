package launch

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"io"

	json "encoding/json/v2"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"testing/iotest"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
)

func TestHeadlessWrapRedirectedEntryPoint(t *testing.T) {
	dir, workspace := installHeadlessFixture(t)
	t.Setenv("MEKUGI_AUTO_WRAP_TRUST", "true")
	ctx, cancel := context.WithTimeout(t.Context(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestAutoWrapProcess$", "--", "router")
	prompt := "  /literal prompt\n!not a shell command\n"
	t.Setenv("MEKUGI_AUTO_WRAP_PROMPT", prompt)
	cmd.Dir, cmd.Stdin = workspace, strings.NewReader(prompt)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	output, err := cmd.Output()
	if err != nil {
		methods, _ := os.ReadFile(filepath.Join(dir, "rpc"))
		t.Fatalf("headless: %v\n%s\n%s\n%s", err, &stderr, output, methods)
	}
	scanner := bufio.NewScanner(bytes.NewReader(output))
	var last string
	for scanner.Scan() {
		var event appserver.Message
		if err := json.Unmarshal(scanner.Bytes(), &event); err != nil {
			t.Fatalf("non-JSON stdout: %q: %v", scanner.Text(), err)
		}
		last = event.Method
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	if last != "mekugi/headless/completed" {
		t.Fatalf("missing terminal event: %s", output)
	}
	methods, err := os.ReadFile(filepath.Join(dir, "rpc"))
	const startup = "initialize\ninitialized\nenvironment/info\nthread/start\nturn/start\n"
	tail, ok := strings.CutPrefix(string(methods), startup)
	// Started/completed native facts invalidate the same coalesced observation.
	// A response between them can require a second read, never a new model turn.
	if err != nil || !ok || tail != "thread/subtree/list\n" && tail != "thread/subtree/list\nthread/subtree/list\n" {
		t.Fatalf("RPC sequence: %s (%v)", methods, err)
	}
}

func TestHeadlessRejectsUnsupportedAuthorizationAndResume(t *testing.T) {
	for _, args := range [][]string{{"headless"}, {"headless", "--yolo", "resume", "thread"}, {"headless", "--yolo", "prompt"}} {
		code, err := wrapCodex(t.Context(), nil, args)
		if code != 2 || err == nil {
			t.Fatalf("%v: code=%d err=%v", args, code, err)
		}
	}
}

func TestHeadlessRejectsProviderArgumentsBeforeReadingPrompt(t *testing.T) {
	for _, args := range [][]string{
		{"--yolo", "-c", `model_provider="custom"`},
		{"--yolo", "--oss"},
		{"--yolo", "-pwork", "--oss"},
		{"--yolo", "--profile="},
	} {
		input := iotest.ErrReader(errors.New("stdin must not be read"))
		code, err := runHeadless(t.Context(), nil, args, input, io.Discard)
		if code != 2 || err == nil {
			t.Fatalf("%v: code=%d error=%v", args, code, err)
		}
	}
}

type headlessFailureWriter struct {
	err    error
	cancel context.CancelFunc
}

func (w headlessFailureWriter) Write(p []byte) (int, error) {
	if w.cancel != nil {
		w.cancel()
		return len(p), nil
	}
	return 0, w.err
}

func TestHeadlessAdmittedOutputFailureAndCancellation(t *testing.T) {
	for _, scenario := range []string{"output", "cancel"} {
		t.Run(scenario, func(t *testing.T) {
			dir, _ := installHeadlessFixture(t)
			ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
			defer cancel()
			failure := errors.New("output failed")
			writer := headlessFailureWriter{err: failure}
			if scenario == "cancel" {
				writer.cancel = cancel
			}
			code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader("literal"), writer)
			if code != 1 || err == nil {
				t.Fatalf("code=%d err=%v", code, err)
			}
			if scenario == "output" && !errors.Is(err, failure) || scenario == "cancel" && !errors.Is(err, context.Canceled) {
				t.Fatalf("lost failure: %v", err)
			}
			entries, readErr := os.ReadDir(os.Getenv("MEKUGI_RUNTIME_DIR"))
			if readErr != nil || len(entries) != 0 {
				t.Fatalf("runtime resources survived: %v, %v", entries, readErr)
			}
			if _, err := os.Stat(filepath.Join(dir, "rpc.joined")); err != nil {
				t.Fatalf("host cleanup not joined: %v", err)
			}
			methods, readErr := os.ReadFile(filepath.Join(dir, "rpc"))
			if readErr != nil || scenario == "output" && bytes.Contains(methods, []byte("thread/start")) {
				t.Fatalf("interpreted undelivered response: %s (%v)", methods, readErr)
			}
		})
	}
}

func TestHeadlessAdmittedFailures(t *testing.T) {
	for _, scenario := range []string{"startup", "approval", "shell-guard", "question", "disconnect", "failed", "interrupted"} {
		t.Run(scenario, func(t *testing.T) {
			dir, workspace := installHeadlessFixture(t)
			t.Setenv("MEKUGI_AUTO_WRAP_HEADLESS_SCENARIO", scenario)
			ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestAutoWrapProcess$", "--", "router")
			cmd.Dir, cmd.Stdin = workspace, strings.NewReader("literal prompt")
			var stderr bytes.Buffer
			cmd.Stderr = &stderr
			output, err := cmd.Output()
			if err == nil || bytes.Contains(output, []byte("mekugi/headless/completed")) {
				t.Fatalf("accepted %s: err=%v output=%s", scenario, err, output)
			}
			want := scenario
			switch scenario {
			case "failed":
				want = "errored"
			case "startup":
				want = "fixture: invalid model configuration"
			case "disconnect":
				want = "fixture: host transport ended"
			case "approval", "shell-guard", "question":
				want = "continue interactively"
			}
			if !strings.Contains(stderr.String(), want) {
				t.Fatalf("lost %s diagnostic: %s", scenario, &stderr)
			}
			if bytes.Contains(output, []byte("fixture:")) {
				t.Fatalf("host stderr leaked into JSONL: %s", output)
			}
			for _, row := range bytes.Split(bytes.TrimSpace(output), []byte("\n")) {
				if len(row) == 0 {
					continue
				}
				var message appserver.Message
				if err := json.Unmarshal(row, &message); err != nil {
					t.Fatalf("non-JSON stdout: %s", row)
				}
			}
			if scenario != "startup" && scenario != "disconnect" {
				if _, err := os.Stat(filepath.Join(dir, "rpc.joined")); err != nil {
					t.Fatalf("host shutdown not joined: %v", err)
				}
			}
		})
	}
}

func installHeadlessFixture(t *testing.T) (string, string) {
	t.Helper()
	dir, workspace := t.TempDir(), t.TempDir()
	t.Setenv("MEKUGI_AUTO_WRAP_PROCESS", "1")
	t.Setenv("MEKUGI_AUTO_WRAP_HEADLESS", "1")
	t.Setenv("MEKUGI_AUTO_WRAP_GUARD", "off")
	t.Setenv("MEKUGI_AUTO_WRAP_FAILURE", "0")
	t.Setenv("MEKUGI_AUTO_WRAP_RPC_MARKER", filepath.Join(dir, "rpc"))
	t.Setenv("MEKUGI_AUTO_WRAP_WORKSPACE", workspace)
	t.Setenv("CODEX_HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	t.Setenv("MEKUGI_RUNTIME_DIR", t.TempDir())
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	script := "#!/bin/sh\nexec '" + strings.ReplaceAll(os.Args[0], "'", "'\\''") + "' -test.run=^TestAutoWrapProcess$ -- codex \"$@\"\n"
	if err := os.WriteFile(filepath.Join(dir, "mekugi-app-server"), []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	return dir, workspace
}

func TestHeadlessLiteralCommandText(t *testing.T) {
	for _, prompt := range []string{"/compact", "/compact\n", "/quit", "!pwd"} {
		t.Run(strings.ReplaceAll(prompt, "\n", "newline"), func(t *testing.T) {
			dir, workspace := installHeadlessFixture(t)
			t.Chdir(workspace)
			t.Setenv("MEKUGI_AUTO_WRAP_PROMPT", prompt)
			ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
			defer cancel()
			var output bytes.Buffer
			code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader(prompt), &output)
			if code != 0 || err != nil || !strings.Contains(output.String(), "mekugi/headless/completed") {
				t.Fatalf("literal prompt %q: code=%d err=%v\n%s", prompt, code, err, &output)
			}
			methods, err := os.ReadFile(filepath.Join(dir, "rpc"))
			if err != nil || bytes.Contains(methods, []byte("thread/compact/start")) || bytes.Count(methods, []byte("turn/start\n")) != 1 {
				t.Fatalf("literal prompt dispatched as command: %s (%v)", methods, err)
			}
		})
	}
}

func TestHeadlessCorrelatedSubmissionFailure(t *testing.T) {
	for _, scenario := range []string{"rpc-rejected", "rpc-stale"} {
		t.Run(scenario, func(t *testing.T) {
			dir, workspace := installHeadlessFixture(t)
			t.Chdir(workspace)
			t.Setenv("MEKUGI_AUTO_WRAP_HEADLESS_SCENARIO", scenario)
			ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
			defer cancel()
			var output bytes.Buffer
			code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader("ordinary prompt"), &output)
			if scenario == "rpc-rejected" {
				if code != 1 || err == nil || !strings.Contains(err.Error(), "turn/start: fixture: start rejected") || errors.Is(err, context.DeadlineExceeded) || strings.Contains(output.String(), "mekugi/headless/completed") {
					t.Fatalf("lost correlated rejection: code=%d err=%v\n%s", code, err, &output)
				}
				if !strings.Contains(output.String(), "fixture: start rejected") {
					t.Fatal("rejection was interpreted before its raw envelope was emitted")
				}
			} else if code != 0 || err != nil || !strings.Contains(output.String(), "mekugi/headless/completed") {
				t.Fatalf("uncorrelated rejection changed submission: code=%d err=%v\n%s", code, err, &output)
			}
			methods, readErr := os.ReadFile(filepath.Join(dir, "rpc"))
			if readErr != nil || bytes.Count(methods, []byte("turn/start\n")) != 1 {
				t.Fatalf("submission was repeated: %s (%v)", methods, readErr)
			}
			if _, err := os.Stat(filepath.Join(dir, "rpc.joined")); err != nil {
				t.Fatalf("host shutdown was not joined: %v", err)
			}
		})
	}
}

func TestHeadlessPreservesNoGuardPolicy(t *testing.T) {
	for _, tc := range []struct {
		name string
		args []string
	}{
		{"default", []string{"--yolo"}},
		{"override-after-explicit-on", []string{"--yolo", "-c", "features.shell_guard=true"}},
		{"override-after-enable-flag", []string{"--yolo", "--enable", "shell_guard"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir, workspace := installHeadlessFixture(t)
			t.Chdir(workspace)
			argsMarker := filepath.Join(dir, "args")
			t.Setenv("MEKUGI_AUTO_WRAP_ARGS_MARKER", argsMarker)
			ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
			defer cancel()
			var output bytes.Buffer
			code, err := runHeadless(ctx, nil, tc.args, strings.NewReader("ordinary prompt"), &output)
			if code != 0 || err != nil {
				t.Fatalf("headless acquired interactive guards: code=%d err=%v", code, err)
			}
			rawArgs, readErr := os.ReadFile(argsMarker)
			if readErr != nil {
				t.Fatalf("missing host args marker: %v", readErr)
			}
			args := strings.Split(strings.TrimSpace(string(rawArgs)), "\n")
			lastDisable, lastEnable, delim := -1, -1, len(args)
			for i, arg := range args {
				if arg == "--" && delim == len(args) {
					delim = i
				}
				if i < delim && i > 0 && args[i-1] == "-c" {
					if arg == "features.shell_guard=false" {
						lastDisable = i
					}
					if arg == "features.shell_guard=true" {
						lastEnable = i
					}
				}
			}
			if lastDisable < 0 || lastDisable <= lastEnable {
				t.Fatalf("headless did not force features.shell_guard=false after user overrides: %q", args)
			}
		})
	}
}

func TestHeadlessWaitsForFamilyJoinBeforeCompleted(t *testing.T) {
	dir, workspace := installHeadlessFixture(t)
	t.Chdir(workspace)
	t.Setenv("MEKUGI_AUTO_WRAP_HEADLESS_SCENARIO", "family-join")
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	var output bytes.Buffer
	code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader("delegate work"), &output)
	if code != 0 || err != nil {
		t.Fatalf("family join failed: code=%d err=%v\n%s", code, err, &output)
	}
	var methods []string
	childCompletedIdx, completedIdx := -1, -1
	for i, row := range bytes.Split(bytes.TrimSpace(output.Bytes()), []byte("\n")) {
		var message appserver.Message
		if err := json.Unmarshal(row, &message); err != nil {
			t.Fatalf("non-JSON stdout: %q: %v", row, err)
		}
		methods = append(methods, message.Method)
		if message.Method == "turn/completed" && bytes.Contains(message.Params, []byte("child-1")) {
			childCompletedIdx = i
		}
		if message.Method == "mekugi/headless/completed" {
			completedIdx = i
		}
	}
	if childCompletedIdx < 0 || completedIdx < 0 || childCompletedIdx >= completedIdx || completedIdx != len(methods)-1 {
		t.Fatalf("completed emitted before family joined: childCompleted=%d completed=%d total=%d\n%s", childCompletedIdx, completedIdx, len(methods), &output)
	}
	if _, err := os.Stat(filepath.Join(dir, "rpc.joined")); err != nil {
		t.Fatalf("host shutdown not joined before completion: %v", err)
	}
}

func TestHeadlessDrainsDelegateEventsAfterTerminalSubtree(t *testing.T) {
	dir, workspace := installHeadlessFixture(t)
	t.Chdir(workspace)
	t.Setenv("MEKUGI_AUTO_WRAP_HEADLESS_SCENARIO", "late-delegate-event")
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	var output bytes.Buffer
	code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader("delegate work"), &output)
	if code != 0 || err != nil {
		t.Fatalf("late delegate drain failed: code=%d err=%v\n%s", code, err, &output)
	}
	var methods []string
	childCompletedIdx, completedIdx := -1, -1
	for i, row := range bytes.Split(bytes.TrimSpace(output.Bytes()), []byte("\n")) {
		var message appserver.Message
		if err := json.Unmarshal(row, &message); err != nil {
			t.Fatalf("non-JSON stdout: %q: %v", row, err)
		}
		methods = append(methods, message.Method)
		if message.Method == "turn/completed" && bytes.Contains(message.Params, []byte("child-1")) {
			childCompletedIdx = i
		}
		if message.Method == "mekugi/headless/completed" {
			completedIdx = i
		}
	}
	if childCompletedIdx < 0 || completedIdx < 0 || childCompletedIdx >= completedIdx || completedIdx != len(methods)-1 {
		t.Fatalf("dropped trailing delegate event: childCompleted=%d completed=%d total=%d\n%s", childCompletedIdx, completedIdx, len(methods), &output)
	}
	if _, err := os.Stat(filepath.Join(dir, "rpc.joined")); err != nil {
		t.Fatalf("host shutdown not joined: %v", err)
	}
}

func TestHeadlessRetriesAfterSubtreeRPCError(t *testing.T) {
	dir, workspace := installHeadlessFixture(t)
	t.Chdir(workspace)
	t.Setenv("MEKUGI_AUTO_WRAP_HEADLESS_SCENARIO", "subtree-retry")
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	var output bytes.Buffer
	code, err := runHeadless(ctx, nil, []string{"--yolo"}, strings.NewReader("ordinary prompt"), &output)
	if code != 0 || err != nil || !strings.Contains(output.String(), "mekugi/headless/completed") {
		t.Fatalf("did not recover from transient subtree error: code=%d err=%v\n%s", code, err, &output)
	}
	if _, err := os.Stat(filepath.Join(dir, "rpc.joined")); err != nil {
		t.Fatalf("host shutdown not joined: %v", err)
	}
}

func TestHeadlessOutputCleanupFailurePreventsCompletion(t *testing.T) {
	_, workspace := installHeadlessFixture(t)
	t.Chdir(workspace)
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	cleanupErr := errors.New("capture flush failed")
	var output bytes.Buffer
	code, err := runHeadlessSession(ctx, headlessSessionConfig{
		args:      []string{"--yolo"},
		workspace: workspace,
		prompt:    "ordinary prompt",
		output:    &output,
		finalize:  func() error { return cleanupErr },
	})
	if code != 1 || !errors.Is(err, cleanupErr) {
		t.Fatalf("cleanup failure did not fail exit: code=%d err=%v", code, err)
	}
	if strings.Contains(output.String(), "mekugi/headless/completed") {
		t.Fatalf("emitted completed despite cleanup failure:\n%s", &output)
	}
}

func TestEvaluateHeadlessFamily(t *testing.T) {
	cases := []struct {
		name          string
		raw           string
		wantDone      bool
		wantRootDone  bool
		wantErrSubstr string
	}{
		{
			name:         "root running",
			raw:          `{"data":[{"id":"root","agentStatus":"running"}]}`,
			wantDone:     false,
			wantRootDone: false,
		},
		{
			name:         "root completed with running child",
			raw:          `{"data":[{"id":"root","agentStatus":"completed"},{"id":"child","parentThreadId":"root","agentStatus":"running"}]}`,
			wantDone:     false,
			wantRootDone: true,
		},
		{
			name:         "root completed with pending_init child",
			raw:          `{"data":[{"id":"root","agentStatus":"completed"},{"id":"child","parentThreadId":"root","agentStatus":"pending_init"}]}`,
			wantDone:     false,
			wantRootDone: true,
		},
		{
			name:         "root and children completed",
			raw:          `{"data":[{"id":"root","agentStatus":"completed"},{"id":"child","parentThreadId":"root","agentStatus":"completed"}]}`,
			wantDone:     true,
			wantRootDone: true,
		},
		{
			name:          "root errored",
			raw:           `{"data":[{"id":"root","agentStatus":"errored"}]}`,
			wantErrSubstr: "errored",
		},
		{
			name:          "root interrupted",
			raw:           `{"data":[{"id":"root","agentStatus":"interrupted"}]}`,
			wantErrSubstr: "interrupted",
		},
		{
			name:          "root shutdown",
			raw:           `{"data":[{"id":"root","agentStatus":"shutdown"}]}`,
			wantErrSubstr: "shutdown",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			done, rootDone, err := evaluateHeadlessFamily("root", []byte(tc.raw))
			if tc.wantErrSubstr != "" {
				if err == nil || !strings.Contains(err.Error(), tc.wantErrSubstr) {
					t.Fatalf("err=%v, want substring %q", err, tc.wantErrSubstr)
				}
				return
			}
			if err != nil || done != tc.wantDone || rootDone != tc.wantRootDone {
				t.Fatalf("got done=%v rootDone=%v err=%v; want done=%v rootDone=%v", done, rootDone, err, tc.wantDone, tc.wantRootDone)
			}
		})
	}
}
