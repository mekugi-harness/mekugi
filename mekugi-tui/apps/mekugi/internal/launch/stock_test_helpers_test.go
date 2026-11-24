package launch

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"slices"
	"strings"
	"syscall"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/recordtransactions"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
)

func mustTestJSON(t *testing.T, value any) []byte {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}

func newManagedMekugiProxy(t *testing.T) *observationFixture {
	t.Helper()
	return newProxyWithSharedTestRegistry(t, sharedProxyTestRegistry(t))
}

func attachTestReplayStore(t *testing.T, proxy *observationFixture) {
	t.Helper()
	store, err := openMekugiReplayStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		// Preview cancellation alone can leave an in-flight snapshot comparison.
		proxy.execWindows.Retire()
		store.snapshots.Close()
	})
	setTestProxyStore(proxy, store)
}

type shellWorkerTestInvocation struct {
	directory   string
	environment []string
}

func runShellWorkerTest(
	t *testing.T,
	registry *frontends.Registry,
	interpreter string,
	interpreterArguments []string,
	script string,
	stdin *os.File,
	invocations ...shellWorkerTestInvocation,
) (stdout, stderr string, exitCode int) {
	t.Helper()
	if len(invocations) > 1 {
		t.Fatal("runShellWorkerTest accepts at most one invocation")
	}
	directory, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	environment := append(os.Environ(), "CODEX_THREAD_ID=", routerTestWorkerEnvironment+"=1")
	if len(invocations) == 1 {
		directory = invocations[0].directory
		environment = append(slices.Clone(invocations[0].environment), routerTestWorkerEnvironment+"=1")
	}
	environment = prependToolFrontendPath(environment, registry.FrontendDirectory)
	arguments := append(append([]string{}, interpreterArguments...), "-c", script)
	command := exec.CommandContext(t.Context(), execTrackShellExecutable(t, interpreter), arguments...)
	command.Dir, command.Env, command.Stdin = directory, environment, stdin
	var out, diagnostic bytes.Buffer
	command.Stdout, command.Stderr = &out, &diagnostic
	runErr := command.Run()
	if runErr != nil {
		exit, ok := errors.AsType[*exec.ExitError](runErr)
		if !ok {
			t.Fatal(runErr)
		}
		exitCode = exit.ExitCode()
		if status, ok := exit.Sys().(syscall.WaitStatus); ok && status.Signaled() {
			exitCode = 128 + int(status.Signal())
		}
	}
	return out.String(), diagnostic.String(), exitCode
}

func prependToolFrontendPath(environment []string, frontendDirectory string) []string {
	result := make([]string, 0, len(environment)+1)
	path := ""
	for _, entry := range environment {
		if value, ok := strings.CutPrefix(entry, "PATH="); ok {
			path = value
			continue
		}
		result = append(result, entry)
	}
	return append(result, "PATH="+frontendDirectory+string(os.PathListSeparator)+path)
}

func bindTestHandleScope(t *testing.T, store *mekugiReplayStore, ctx context.Context, parent, fork string) context.Context {
	t.Helper()
	thread := store.scoped(ctx).session.Thread
	selected := store.scoped(ctx)
	namespace := selected.session.Namespace
	if parent != "" {
		var err error
		namespace, err = store.publication.NamespaceForThread(parent)
		if err != nil {
			t.Fatal(err)
		}
	}
	selected.session.Namespace = namespace
	if err := recordtransactions.RetainInput(ctx, selected.changeScope(), selected.changes, recordtransactions.Input{Workspace: "/w", Initialize: &history.Scope{Thread: thread, Namespace: namespace, Parent: parent, Fork: fork}}); err != nil {
		t.Fatal(err)
	}
	ctx = context.WithValue(ctx, storageSessionKey{}, selected.session)
	return ctx
}
