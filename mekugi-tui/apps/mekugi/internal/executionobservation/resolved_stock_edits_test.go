package executionobservation

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/legacycall"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

func TestSupportedSnapshotGeneratorRetainsInitialCreation(t *testing.T) {
	t.Parallel()
	f := newMChangesSliceFixture(t, "snapshot-generator")
	pythonName, pythonBinary := interpreterForTest("python3", "python")
	if pythonBinary == "" {
		t.Skip("Python runtime unavailable")
	}
	command := pythonName + " <<'PY'\nfrom pathlib import Path\nout = Path('testdata/snapshots')\nout.mkdir(parents=True, exist_ok=True)\nfixtures = {'new.txt': 'first\\n', 'other.txt': 'body\\n'}\nfor name, body in fixtures.items():\n    (out / name).write_text(body)\nPY"
	observation := observeTestCommand(t, f.workspace, command)
	if _, err := os.Stat(filepath.Join(f.workspace, "testdata")); !os.IsNotExist(err) {
		t.Fatal("capture executed the generator")
	}
	host := exec.Command("bash", "-c", command)
	host.Dir = f.workspace
	if output, err := host.CombinedOutput(); err != nil {
		t.Fatalf("host generator: %v: %s", err, output)
	}
	proxy := testProxyWithStore(f.store)
	history := mekugiHistory{ToolName: nativeExecCommandToolName, ExecutingThread: "generator-agent", ExecObservation: observation}
	if err := finalizeExecObservations(proxy.Retained, proxy.Changes, proxy.Checkpoints, proxy.PublishChanges, proxy.Windows, proxy.Audience, proxy.Notice, f.ctx, f.workspace, []execCompletion{{callID: "generate", history: history, result: binding.ExecutionResult{Terminal: true, Succeeded: true, ExitCode: new(0)}}}); err != nil {
		t.Fatal(err)
	}
	record, found, err := f.store.lookup(f.ctx, f.workspace, legacycall.ExecCallID("generate", false))
	if err != nil || !found || len(record.ReviewFiles) != 2 || record.ChangeID == "" {
		t.Fatalf("generator capture missing: %+v, %v", record, err)
	}
	for _, file := range record.ReviewFiles {
		if file.BeforePath != "" || file.Incomplete != "" {
			t.Fatalf("generator creation baseline missing: %+v", file)
		}
	}
	net, err := f.store.changes.Read(f.ctx, f.store.scoped(f.ctx).changeScope(), changes.Query{Workspace: f.workspace, IDs: []string{record.ChangeID}, View: "net"})
	if err != nil || !strings.Contains(net, "+first") || !strings.Contains(net, "+body") {
		t.Fatalf("generator net: %q, %v", net, err)
	}
}

func TestPythonTemporaryWriteAndRestoreRetainsIncompleteHistory(t *testing.T) {
	t.Parallel()
	f := newMChangesSliceFixture(t, "write-restore")
	pythonName, pythonBinary := interpreterForTest("python3", "python")
	if pythonBinary == "" {
		t.Skip("Python runtime unavailable")
	}
	path := filepath.Join(f.workspace, "target.go")
	current := "package fixture\n// current\n"
	writeTestFile(t, path, current)
	// The screenshot's producer shape, in an isolated fixture. The nested
	// command checks the temporary bytes while finally restores the original.
	command := pythonName + " <<'PY'\nimport pathlib, subprocess\npath = pathlib.Path('target.go')\ncurrent = path.read_text()\nreplacement = current.replace('current', 'baseline')\ntry:\n    path.write_text(replacement)\n    subprocess.run(['python3', '-c', \"from pathlib import Path; assert 'baseline' in Path('target.go').read_text()\"], check=True)\nfinally:\n    path.write_text(current)\nPY"
	observation := observeTestCommand(t, f.workspace, command)
	if !observation.RepeatedPaths {
		t.Fatal("temporary write-and-restore scope not identified")
	}
	before, err := os.ReadFile(path)
	if err != nil || string(before) != current {
		t.Fatal("observation executed the Python script")
	}
	host := exec.Command("bash", "-c", command)
	host.Dir = f.workspace
	if output, err := host.CombinedOutput(); err != nil {
		t.Fatalf("host: %v: %s", err, output)
	}
	after, err := os.ReadFile(path)
	if err != nil || string(after) != current {
		t.Fatal("fixture did not restore its final bytes")
	}
	proxy := testProxyWithStore(f.store)
	history := mekugiHistory{ToolName: nativeExecCommandToolName, ExecutingThread: f.thread, ExecObservation: observation}
	output := "Wall time: 0.01 seconds\nProcess exited with code 0\nFinal output:\n"
	if err := finalizeExecObservations(proxy.Retained, proxy.Changes, proxy.Checkpoints, proxy.PublishChanges, proxy.Windows, proxy.Audience, proxy.Notice, f.ctx, f.workspace, []execCompletion{{callID: "temporary", history: history, result: binding.ExecutionResult{Terminal: true, Succeeded: true, ExitCode: new(0), Text: output}}}); err != nil {
		t.Fatal(err)
	}
	record, found, err := f.store.lookup(f.ctx, f.workspace, legacycall.ExecCallID("temporary", false))
	if err != nil || !found || record.ChangeID != "" || len(record.ReviewFiles) != 0 {
		t.Fatalf("attempt vanished or fabricated intermediate diff: %+v, %v", record, err)
	}
	if record.ExecOutcome.Coverage != changecapture.CoveragePartial || !strings.Contains(record.ExecOutcome.ScopeReason, "intermediate writes unobserved") {
		t.Fatal("temporary edit uncertainty was not retained in command diagnostics")
	}

}

func TestMChangesSummaryComposesManagedAndDirectEdits(t *testing.T) {
	t.Parallel()
	f := newMChangesSliceFixture(t, "summary-composition")
	path := filepath.Join(f.workspace, "file.txt")
	for i, origin := range []string{"gofmt", "", "gofmt"} {
		before, after := "", "one\n"
		if i == 1 {
			before, after = "one\n", "two\nextra\n"
		}
		if i == 2 {
			before, after = "two\nextra\n", "final\n"
		}
		correlation := fmt.Sprint(i)
		id := f.reserve(t, f.thread, correlation)
		file := diffmodel.RenderReviewFile(path, path, before, after)
		if i == 0 {
			file = diffmodel.RenderReviewFile("", path, before, after)
		}
		file.Origin = origin
		f.publish(t, id, correlation, correlation, mekugiHistory{ReviewFiles: []diffmodel.ReviewFile{file}})
	}
	summary, err := f.store.changes.Read(f.ctx, f.store.scoped(f.ctx).changeScope(), changes.Query{Workspace: f.workspace, Mine: true, View: "summary"})
	if err != nil || summary != "M\t2\t1\tfile.txt\n" {
		t.Fatalf("summary summed attempts or split ownership: %q, %v", summary, err)
	}
}

func TestMChangesReadContextDistinguishesMissingEmptyAndPathScope(t *testing.T) {
	t.Parallel()
	f := newMChangesSliceFixture(t, "read-context")
	id := f.reserve(t, "worker", "worker-effect")
	f.publish(t, id, "worker-effect", "worker-call", mekugiHistory{ReviewFiles: []diffmodel.ReviewFile{diffmodel.RenderReviewFile("file.txt", "file.txt", "old\n", "new\n")}})
	output, stderr, status := f.run(t, "mchanges "+id+" --net")
	if status != 0 || stderr != "" || !strings.Contains(output, "+new") {
		t.Fatalf("explicit worker capture unreadable: %q, %q, %d", output, stderr, status)
	}
	output, stderr, status = f.run(t, "mchanges --mine --net")
	if status != 0 || stderr != "" || !strings.Contains(output, "no captures selected") || !strings.Contains(output, f.workspace) {
		t.Fatalf("own-thread empty selection ambiguous: %q, %q, %d", output, stderr, status)
	}
	output, stderr, status = f.run(t, "mchanges "+id+" --net -- missing.txt")
	if status != 0 || stderr != "" || !strings.Contains(output, "no captured files match") {
		t.Fatalf("path mismatch became empty net: %q, %q, %d", output, stderr, status)
	}
	gap := f.reserve(t, f.thread, "gap")
	f.publish(t, gap, "gap", "gap-call", mekugiHistory{ExecOutcome: &changecapture.Outcome{Coverage: changecapture.CoveragePartial, ScopeReason: "host inputs missing"}})
	output, stderr, status = f.run(t, "mchanges "+gap+" --net")
	if status == 0 || !strings.Contains(stderr, "partial captured effects") || strings.Contains(output, "no net changes") {
		t.Fatalf("missing evidence became empty successful net: %q, %q, %d", output, stderr, status)
	}
	output, stderr, status = f.run(t, "mchanges --list "+gap)
	if status != 0 || stderr != "" || !strings.Contains(output, "?") {
		t.Fatalf("gap ID disappeared from inventory: %q, %q, %d", output, stderr, status)
	}
}

func TestMChangesFilteredZeroNetIsNotPathMismatch(t *testing.T) {
	t.Parallel()
	for _, creation := range []bool{false, true} {
		t.Run(fmt.Sprintf("creation=%v", creation), func(t *testing.T) {
			f := newMChangesSliceFixture(t, "filtered-zero-net")
			path := filepath.Join(f.workspace, "file.txt")
			files := []diffmodel.ReviewFile{diffmodel.RenderReviewFile(path, path, "old\n", "new\n"), diffmodel.RenderReviewFile(path, path, "new\n", "old\n")}
			if creation {
				files = []diffmodel.ReviewFile{diffmodel.RenderReviewFile("", path, "", "new\n"), diffmodel.RenderReviewFile(path, "", "new\n", "")}
			}
			var ids []string
			for i, file := range files {
				call := fmt.Sprint(i)
				id := f.reserve(t, f.thread, call)
				ids = append(ids, id)
				f.publish(t, id, call, call, mekugiHistory{ReviewFiles: []diffmodel.ReviewFile{file}})
			}
			output, err := f.store.changes.Read(f.ctx, f.store.scoped(f.ctx).changeScope(), changes.Query{Workspace: f.workspace, IDs: ids, View: "net", Paths: []string{"file.txt"}})
			if err != nil || output != "no net changes in selected captured history\n" {
				t.Fatalf("matching zero net misreported: %q, %v", output, err)
			}
		})
	}
}
