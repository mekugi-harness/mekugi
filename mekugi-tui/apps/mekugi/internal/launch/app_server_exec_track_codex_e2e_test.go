//go:build codex_e2e

package launch

import (
	"crypto/sha256"
	json "encoding/json/v2"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/charmbracelet/x/vt"
)

func TestAppServerNativeMRunAndPTY(t *testing.T) {
	workspace, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	bash := execTrackShellExecutable(t, "bash")
	command := `test "$(command -v mrun)" = "$CODEX_ARG0_PATH_DIR/mrun" || exit 1
printf 'piped input\n' | mrun --max-tokens 1024 -- sh -c 'printf "%s|%s|%s|%s|" "$PWD" "$MEKUGI_FRONTEND_TEST" "$1" "$2"; cat' sh one 'two words'`
	provider := &toolFrontendCodexProvider{
		program: fmt.Sprintf(`// @exec: {"yield_time_ms": 30000}
const first = await tools.exec_command({cmd:%s,shell:%s,login:false,workdir:%s});
if (first.exit_code !== 0) throw new Error(JSON.stringify(first));
text(first.output);
const started = await tools.exec_command({cmd:"IFS= read -r value; printf 'continued:%%s\\n' \"$value\"",shell:%s,login:false,tty:true,yield_time_ms:250});
if (typeof started.session_id !== "number") throw new Error("PTY did not yield");
const completed = await tools.write_stdin({session_id:started.session_id,chars:"codex-input\n",yield_time_ms:5000});
if (completed.exit_code !== 0) throw new Error(JSON.stringify(completed));
text(completed.output);
text("HOST_RESULT_VERIFIED");`, e2eMustMarshalJSON(command), e2eMustMarshalJSON(bash), e2eMustMarshalJSON(workspace), e2eMustMarshalJSON(bash)),
		expected:  []string{workspace + "|inherited|one|two words|piped input", "continued:codex-input", "HOST_RESULT_VERIFIED"},
		finalText: "Bundled command and PTY completed.",
	}
	runAppServerPreviewWith(t, provider, nil, appServerPreview{
		managedRuntime: true,
		workspace:      workspace,
		environment:    []string{"SHELL=" + bash, "MEKUGI_FRONTEND_TEST=inherited"},
		codexArgs:      []string{"-c", `sandbox_mode="danger-full-access"`},
		completedText:  provider.finalText,
	})
}

func TestAppServerExecTrackNativeCodex(t *testing.T) {
	bash := execTrackShellExecutable(t, "bash")
	home, workspace := t.TempDir(), t.TempDir()
	marker, hook := filepath.Join(home, "hook-ran"), filepath.Join(home, "rewrite")
	writeHostFixture(t, filepath.Join(workspace, "git"), "#!/bin/sh\ntouch target-ran\n", 0700)
	const command = `printf 'HOOK_%s\n' ONE; ./git push origin main || printf 'HOOK_%s\n' RECOVERED; printf 'HOOK_%s\n' TWO >&2; false && printf 'HOOK_%s\n' NEVER`
	rewrite := e2eMustMarshalJSON(map[string]any{"hookSpecificOutput": map[string]any{
		"hookEventName": "PreToolUse", "permissionDecision": "allow",
		"updatedInput": map[string]string{"command": command},
	}})
	writeHostFixture(t, hook, "#!/bin/sh\ncat >/dev/null\nprintf ran >>"+quoteShellWord(marker)+"\nprintf '%s\\n' "+quoteShellWord(string(rewrite))+"\n", 0700)
	configPath := filepath.Join(home, "config.toml")
	hookCommand := quoteShellWord(hook)
	identity := map[string]any{"event_name": "pre_tool_use", "matcher": "^Bash$", "hooks": []any{
		map[string]any{"type": "command", "command": hookCommand, "timeout": 5, "async": false},
	}}
	encoded, err := json.Marshal(identity, json.Deterministic(true))
	if err != nil {
		t.Fatal(err)
	}
	config := fmt.Sprintf("[hooks]\nPreToolUse = [{matcher='^Bash$',hooks=[{type='command',command=%q,timeout=5}]}]\nstate.%q.trusted_hash='sha256:%x'\n",
		hookCommand, configPath+":pre_tool_use:0:0", sha256.Sum256(encoded))
	writeTestFile(t, configPath, config)
	provider := &toolFrontendCodexProvider{
		program:   hostTrackedCommand(bash, "echo MODEL_ORIGINAL", 1),
		expected:  []string{"HOOK_ONE", "remote write denied: fixture denial", "HOOK_RECOVERED", "HOOK_TWO", "HOST_RESULT_VERIFIED"},
		finalText: "Hooked command recovered after denial.",
	}
	runAppServerPreviewWith(t, provider, nil, appServerPreview{
		managedRuntime: true, workspace: workspace,
		environment:   []string{"SHELL=" + bash, "CODEX_HOME=" + home},
		codexArgs:     []string{"-c", `sandbox_mode="danger-full-access"`},
		completedText: provider.finalText,
		duringTurn: func(t *testing.T, outer io.Writer, await func(string), _ func(func(string) bool), _ *vt.Emulator) {
			await("Approve guarded shell command")
			if _, err := io.WriteString(outer, "fixture denial\r"); err != nil {
				t.Fatal(err)
			}
		},
		afterPrompt: func(t *testing.T, outer io.Writer, _ func(string), awaitFrame func(func(string) bool), _ *vt.Emulator) {
			expandHostActivity(t, outer)
			awaitFrame(func(frame string) bool {
				// These are authored steps, not individual execution records.
				return hostRowsInOrder(frame, `printf 'HOOK_%s\n' ONE`, `./git push origin main`,
					`printf 'HOOK_%s\n' RECOVERED`, `printf 'HOOK_%s\n' TWO >&2`, "false", `printf 'HOOK_%s\n' NEVER`,
					"combined output · exit 1") && strings.Contains(frame, "┆ HOOK_RECOVERED") &&
					strings.Contains(frame, "┆ HOOK_TWO")
			})
		},
	})
	assertHostFile(t, marker, "ran")
	if _, err := os.Stat(filepath.Join(workspace, "target-ran")); !os.IsNotExist(err) {
		t.Fatalf("denied command ran: %v", err)
	}
	provider.mu.Lock()
	defer provider.mu.Unlock()
	if strings.Contains(provider.output, "MODEL_ORIGINAL") || strings.Contains(provider.output, "HOOK_NEVER") {
		t.Fatalf("a replaced or short-circuited command produced output: %q", provider.output)
	}
}

func TestAppServerExecTrackNativeCodexDash(t *testing.T) {
	resolved, err := filepath.EvalSymlinks("/bin/sh")
	if err != nil || filepath.Base(resolved) != "dash" {
		t.Skip("requires dash-backed /bin/sh")
	}
	workspace := t.TempDir()
	gate := filepath.Join(workspace, "continue")
	const command = `printf '%s_%s\n' DASH FIRST; until test -f continue; do sleep .05; done; printf '%s_%s\n' DASH SECOND >&2; false && touch skipped`
	provider := &toolFrontendCodexProvider{
		program:   hostTrackedCommand("/bin/sh", command, 1),
		expected:  []string{"DASH_FIRST", "DASH_SECOND", "HOST_RESULT_VERIFIED"},
		finalText: "Dash compound command completed.",
	}
	runAppServerPreviewWith(t, provider, nil, appServerPreview{
		managedRuntime: true, workspace: workspace,
		environment: []string{"SHELL=/bin/sh"}, codexArgs: []string{"-c", `sandbox_mode="danger-full-access"`},
		completedText: provider.finalText,
		duringTurn: func(t *testing.T, outer io.Writer, _ func(string), awaitFrame func(func(string) bool), _ *vt.Emulator) {
			expandHostActivity(t, outer)
			awaitFrame(func(frame string) bool {
				return hostRowsInOrder(frame, "Running combined output", "┆ DASH_FIRST")
			})
			writeHostFixture(t, gate, "", 0600)
		},
		afterPrompt: func(t *testing.T, outer io.Writer, _ func(string), awaitFrame func(func(string) bool), screen *vt.Emulator) {
			awaitFrame(func(frame string) bool {
				return strings.Count(frame, "combined output") == 1 && hostRowsInOrder(frame, `printf '%s_%s\n' DASH FIRST`, `printf '%s_%s\n' DASH SECOND >&2`,
					"false", "touch skipped", "combined output · exit 1", "┆ DASH_FIRST", "┆ DASH_SECOND")
			})
			for y, row := range strings.Split(screen.String(), "\n") {
				if !strings.Contains(row, "combined output · exit 1") {
					continue
				}
				if _, err := fmt.Fprintf(outer, "\x1b[<0;10;%dM\x1b[<0;10;%dm", y+1, y+1); err != nil {
					t.Fatal(err)
				}
				awaitFrame(func(frame string) bool {
					_, details, ok := strings.Cut(frame, "── ")
					return ok && hostRowsInOrder(details, "┆ DASH_FIRST", "┆ DASH_SECOND", "y copy")
				})
				return
			}
			t.Fatalf("no Dash batch output row:\n%s", screen.String())
		},
	})
	if _, err := os.Stat(filepath.Join(workspace, "skipped")); !os.IsNotExist(err) {
		t.Fatalf("short-circuited command ran: %v", err)
	}
}

func TestAppServerExecTrackNativeCodexMovedWorkspace(t *testing.T) {
	bash := execTrackShellExecutable(t, "bash")
	root := shortTempDir(t)
	workspace, moved := filepath.Join(root, "original"), filepath.Join(root, "relocated")
	if err := os.Mkdir(workspace, 0700); err != nil {
		t.Fatal(err)
	}
	writeTestFile(t, filepath.Join(workspace, "workspace.txt"), "Moved workspace evidence.\n")
	const before = `printf 'capture before move\n' > before-move.txt`
	const after = `test -f workspace.txt && printf 'edit after move\n' > after-move.txt; cat workspace.txt; false && touch skipped`
	// The edit under the original workspace stays visible. The new temporary
	// location is outside that workspace and must not join its saved changes.
	provider := &toolFrontendCodexProvider{
		program: fmt.Sprintf(`// @exec: {"yield_time_ms": 30000}
const shell = %s;
const before = await tools.exec_command({cmd:%s,shell,login:false});
if (before.exit_code !== 0) throw new Error(JSON.stringify(before));
const list = {cmd:"mchanges --mine --list",shell,login:false};
const retained = await tools.exec_command(list);
if (retained.exit_code !== 0 || !retained.output.includes("before-move.txt")) throw new Error(JSON.stringify(retained));
const moved = await tools.exec_command({cmd:%s,shell,login:false});
if (moved.exit_code !== 0) throw new Error(JSON.stringify(moved));
const workdir = %s;
const after = await tools.exec_command({cmd:%s,shell,login:false,workdir});
if (after.exit_code !== 1) throw new Error(JSON.stringify(after));
text(after.output);
const listed = await tools.exec_command({...list,workdir});
if (listed.exit_code !== 0 || listed.output !== retained.output) throw new Error(JSON.stringify(listed));
text(listed.output);
text("HOST_RESULT_VERIFIED");`, e2eMustMarshalJSON(bash), e2eMustMarshalJSON(before), e2eMustMarshalJSON(`mv "$PWD" `+quoteShellWord(moved)),
			e2eMustMarshalJSON(moved), e2eMustMarshalJSON(after)),
		expected:  []string{"Moved workspace evidence.", "before-move.txt", "HOST_RESULT_VERIFIED"},
		finalText: "Explicit workspace still works after relocation.",
	}
	runAppServerPreviewWith(t, provider, nil, appServerPreview{
		managedRuntime: true, workspace: workspace,
		environment: []string{"SHELL=" + bash, "TMPDIR=" + root, "MEKUGI_NATIVE_MCHANGES=1"},
		codexArgs:   []string{"-c", `sandbox_mode="danger-full-access"`}, completedText: provider.finalText,
		afterPrompt: func(t *testing.T, outer io.Writer, _ func(string), awaitFrame func(func(string) bool), _ *vt.Emulator) {
			expandHostActivity(t, outer)
			awaitFrame(func(frame string) bool {
				return hostRowsInOrder(frame, "In "+moved, "cat workspace.txt", "false", "touch skipped",
					"combined output · exit 1", "┆ Moved workspace evidence.")
			})
		},
	})
	assertHostFile(t, filepath.Join(moved, "before-move.txt"), "capture before move\n")
	assertHostFile(t, filepath.Join(moved, "after-move.txt"), "edit after move\n")
	if _, err := os.Stat(filepath.Join(moved, "skipped")); !os.IsNotExist(err) {
		t.Fatalf("short-circuited command ran: %v", err)
	}
}

func TestAppServerExecTrackNativeCodexSandbox(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("asserts the Linux sandbox boundary")
	}
	bash := execTrackShellExecutable(t, "bash")
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	// A live listener distinguishes sandbox denial from an unused host port.
	command := fmt.Sprintf(`cd stage && until test -f ready; do sleep .05; done; { echo > /dev/tcp/127.0.0.1/%d; } 2>/dev/null && exit 9; printf 'SANDBOX_%%s\n' ONE; printf 'SANDBOX_%%s\n' TWO >&2; false && printf 'SANDBOX_%%s\n' NEVER`, listener.Addr().(*net.TCPAddr).Port)
	for _, sandbox := range []string{"read-only", "workspace-write"} {
		t.Run(sandbox, func(t *testing.T) {
			workspace := t.TempDir()
			if err := os.Mkdir(filepath.Join(workspace, "stage"), 0700); err != nil {
				t.Fatal(err)
			}
			provider := &toolFrontendCodexProvider{
				program:   hostTrackedCommand(bash, command, 1),
				expected:  []string{"SANDBOX_ONE", "SANDBOX_TWO", "HOST_RESULT_VERIFIED"},
				finalText: "Sandboxed command delivered its output.",
			}
			runAppServerPreviewWith(t, provider, nil, appServerPreview{
				managedRuntime: true, workspace: workspace, environment: []string{"SHELL=" + bash},
				codexArgs:     []string{"-c", fmt.Sprintf("sandbox_mode=%q", sandbox), "-c", `approval_policy="never"`},
				completedText: provider.finalText,
				duringTurn: func(t *testing.T, outer io.Writer, await func(string), _ func(func(string) bool), _ *vt.Emulator) {
					expandHostActivity(t, outer)
					await("Running combined output")
					writeHostFixture(t, filepath.Join(workspace, "stage", "ready"), "", 0600)
				},
				afterPrompt: func(_ *testing.T, _ io.Writer, _ func(string), awaitFrame func(func(string) bool), _ *vt.Emulator) {
					awaitFrame(func(frame string) bool {
						return hostRowsInOrder(frame, "exit 9", `printf 'SANDBOX_%s\n' ONE`, `printf 'SANDBOX_%s\n' TWO >&2`,
							"false", `printf 'SANDBOX_%s\n' NEVER`, "combined output · exit 1") &&
							strings.Contains(frame, "┆ SANDBOX_ONE") && strings.Contains(frame, "┆ SANDBOX_TWO")
					})
				},
			})
			provider.mu.Lock()
			defer provider.mu.Unlock()
			if strings.Contains(provider.output, "SANDBOX_NEVER") {
				t.Fatalf("short-circuited command produced output: %q", provider.output)
			}
		})
	}
}

func expandHostActivity(t *testing.T, outer io.Writer) {
	t.Helper()
	if _, err := io.WriteString(outer, "\x05\x05"); err != nil {
		t.Fatal(err)
	}
}

// Match each substring on a different row, in order. Matching within a single
// compound source line would not exercise native step projection.
func hostRowsInOrder(frame string, rows ...string) bool {
	for row := range strings.Lines(frame) {
		if len(rows) > 0 && strings.Contains(row, rows[0]) {
			rows = rows[1:]
		}
	}
	return len(rows) == 0
}

func hostTrackedCommand(shell, command string, exit int) string {
	return fmt.Sprintf(`// @exec: {"yield_time_ms": 30000}
const result = await tools.exec_command({cmd:%s,shell:%s,login:false,yield_time_ms:25000});
if (result.exit_code !== %d) throw new Error(JSON.stringify(result));
text(result.output);
text("HOST_RESULT_VERIFIED");`, e2eMustMarshalJSON(command), e2eMustMarshalJSON(shell), exit)
}

func writeHostFixture(t *testing.T, path, contents string, mode os.FileMode) {
	t.Helper()
	if err := os.WriteFile(path, []byte(contents), mode); err != nil {
		t.Fatal(err)
	}
}

func assertHostFile(t *testing.T, path, want string) {
	t.Helper()
	if got, err := os.ReadFile(path); err != nil || string(got) != want {
		t.Fatalf("%s = %q, %v; want %q", path, got, err, want)
	}
}
