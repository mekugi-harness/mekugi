//go:build codex_e2e

package launch

import (
	"errors"
	"fmt"
	"net/http"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestMChangesNestedNativeCodexE2E(t *testing.T) {
	workspace, sessionRoot := t.TempDir(), shortTempDir(t)
	bash := execTrackShellExecutable(t, "bash")
	const patch = "*** Begin Patch\n*** Add File: nested-patch.txt\n+patch-result\n*** End Patch\n"
	const rejected = "*** Begin Patch\n*** Update File: nested-patch.txt\n@@\n-absent-before-line\n+replacement\n*** End Patch\n"
	const shellWrite = "printf 'command-result\\n' >nested-command.txt"
	const partialWrite = "printf 'partial-result\\n' >nested-partial.txt; exit 7"
	const yieldedWrite = "printf 'yielded-result\\n' >nested-yielded.txt; IFS= read -r continue"
	var threadID string
	provider := &toolFrontendCodexProvider{
		program: fmt.Sprintf(`// @exec: {"yield_time_ms": 30000}
await tools.apply_patch(%s);
const shell = %s;
const written = await tools.exec_command({cmd:%s,shell,login:false});
if (written.exit_code !== 0) throw new Error(JSON.stringify(written));
let rejected = false;
try { await tools.apply_patch(%s); } catch (_) { rejected = true; }
if (!rejected) throw new Error("invalid patch did not fail");
const partial = await tools.exec_command({cmd:%s,shell,login:false});
if (partial.exit_code !== 7) throw new Error(JSON.stringify(partial));
const started = await tools.exec_command({cmd:%s,shell,login:false,tty:true,yield_time_ms:250});
if (typeof started.session_id !== "number") throw new Error(JSON.stringify(started));
const list = {cmd:"mchanges --mine --list",shell,login:false};
const pending = await tools.exec_command(list);
if (pending.exit_code !== 0 || pending.output.includes("nested-yielded.txt")) throw new Error(JSON.stringify(pending));
const resumed = await tools.write_stdin({session_id:started.session_id,chars:"done\n",yield_time_ms:5000});
if (resumed.exit_code !== 0) throw new Error(JSON.stringify(resumed));
const seen = await tools.exec_command(list);
if (seen.exit_code !== 0) throw new Error(JSON.stringify(seen));
text(seen.output);
text("HOST_RESULT_VERIFIED");`, e2eMustMarshalJSON(patch), e2eMustMarshalJSON(bash), e2eMustMarshalJSON(shellWrite), e2eMustMarshalJSON(rejected), e2eMustMarshalJSON(partialWrite), e2eMustMarshalJSON(yieldedWrite)),
		expected:  []string{"nested-patch.txt", "nested-command.txt", "nested-partial.txt", "nested-yielded.txt", "exit=7", "HOST_RESULT_VERIFIED"},
		finalText: "Nested edits are visible to the author.",
		observeRequest: func(_ []byte, headers http.Header) error {
			metadata, _ := fixtureTurnMetadata(headers)
			if metadata.ThreadID == "" {
				return errors.New("model request omitted its thread ID")
			}
			if threadID != "" && threadID != metadata.ThreadID {
				return errors.New("model request changed thread after nested tools")
			}
			threadID = metadata.ThreadID
			return nil
		},
	}
	if !t.Run("managed runtime", func(t *testing.T) {
		runAppServerPreviewWith(t, provider, nil, appServerPreview{
			managedRuntime: true, workspace: workspace,
			environment:   []string{"SHELL=" + bash, "TMPDIR=" + sessionRoot, "MEKUGI_NATIVE_MCHANGES=1"},
			codexArgs:     []string{"-c", `sandbox_mode="danger-full-access"`},
			completedText: provider.finalText,
		})
	}) {
		return
	}
	assertHostFile(t, filepath.Join(workspace, "nested-patch.txt"), "patch-result\n")
	assertHostFile(t, filepath.Join(workspace, "nested-command.txt"), "command-result\n")
	assertHostFile(t, filepath.Join(workspace, "nested-partial.txt"), "partial-result\n")
	assertHostFile(t, filepath.Join(workspace, "nested-yielded.txt"), "yielded-result\n")
	stores, err := filepath.Glob(filepath.Join(sessionRoot, "mekugi-session-*", "change-history"))
	if err != nil || len(stores) != 1 {
		t.Fatalf("durable native store = %v: %v", stores, err)
	}
	native, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		t.Fatal(err)
	}
	read := func(thread string, args ...string) string {
		t.Helper()
		cmd := exec.CommandContext(t.Context(), native, args...)
		cmd.Args[0] = "mchanges"
		cmd.Dir = workspace
		cmd.Env = append(cmd.Environ(), "CODEX_THREAD_ID="+thread, "MEKUGI_CHANGE_HISTORY_DIR="+stores[0])
		output, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("mchanges %v: %v\n%s", args, err, output)
		}
		return string(output)
	}
	provider.mu.Lock()
	author := threadID
	provider.mu.Unlock()
	list := read(author, "--mine", "--list")
	lines := strings.Split(strings.TrimSpace(list), "\n")
	if len(lines) != 4 {
		t.Fatalf("expected four durable effects, including only the shell's failed effect:\n%s", list)
	}
	var ids []string
	for _, line := range lines {
		id, _, _ := strings.Cut(line, "\t")
		ids = append(ids, id)
	}
	diff := read(author, "--mine")
	for _, want := range []string{"+patch-result", "+command-result", "+partial-result", "+yielded-result"} {
		if !strings.Contains(diff, want) {
			t.Fatalf("author diff lacks %q:\n%s", want, diff)
		}
	}
	other := "00000000-0000-4000-8000-000000000001"
	if got := read(other, "--mine", "--list"); strings.Contains(got, "nested-") {
		t.Fatalf("reviewer's own view leaked author changes:\n%s", got)
	}
	history := read(other, append(ids, "--history")...)
	for _, want := range []string{"[apply_patch]", "[shell_write]", "exit=7", "nested-patch.txt", shellWrite, partialWrite, yieldedWrite} {
		if !strings.Contains(history, want) {
			t.Fatalf("reviewer history lacks %q:\n%s", want, history)
		}
	}
	calls := make(map[string]bool)
	for _, line := range strings.Split(history, "\n") {
		if _, receipt, ok := strings.Cut(line, " thread="+author+" call="); ok {
			call, _, _ := strings.Cut(receipt, " ")
			calls[call] = true
		}
	}
	if len(calls) != len(ids) || calls[""] {
		t.Fatalf("expected a distinct host receipt for each change:\n%s", history)
	}
}
