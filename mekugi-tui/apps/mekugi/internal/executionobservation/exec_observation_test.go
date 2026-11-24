package executionobservation

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	changepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"

	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/projection"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
)

func writeTestFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
}

func observeTestCommand(t *testing.T, workdir, command string) *changecapture.Observation {
	t.Helper()
	observation, observed := changecapture.Capture(t.Context(), []commandanalysis.Command{{Command: command, Workdir: workdir, Shell: "bash"}}, false, false, changecapture.Options{})
	if !observed {
		t.Fatalf("%q was not observed; observation=%+v", command, observation)
	}
	return observation
}

func reviewSummary(workspace string, files []diffmodel.ReviewFile) []string {
	var summary []string
	for _, file := range files {
		text := file.Action().Title() + " " + strings.TrimPrefix(cmpOrPath(file.AfterPath, file.BeforePath), workspace+"/")
		if file.Action() == diffmodel.ReviewMove {
			text = "Move " + strings.TrimPrefix(file.BeforePath, workspace+"/") + " -> " + strings.TrimPrefix(file.AfterPath, workspace+"/")
		}
		if file.CopyFrom != "" {
			text += " (copy of " + strings.TrimPrefix(file.CopyFrom, workspace+"/") + ")"
		}
		if file.Binary {
			text += " binary"
		}
		if file.Incomplete != "" {
			text += " incomplete"
		}
		summary = append(summary, text)
	}
	slices.Sort(summary)
	return summary
}

func cmpOrPath(values ...string) string {
	for _, value := range values {
		if value != "" {
			return value
		}
	}
	return ""
}

func TestExecObservationReconcilesDeclaredEffects(t *testing.T) {
	for _, test := range []struct {
		name    string
		setup   map[string]string
		command string
		run     func(t *testing.T, workspace string)
		want    []string
	}{
		{
			name: "copy", setup: map[string]string{"a.txt": "a\n"}, command: "cp a.txt b.txt",
			run:  func(t *testing.T, w string) { writeTestFile(t, filepath.Join(w, "b.txt"), "a\n") },
			want: []string{"Create b.txt (copy of a.txt)"},
		},
		{
			name: "copy into directory", setup: map[string]string{"a.txt": "a\n", "dir/keep": ""}, command: "cp a.txt dir",
			run:  func(t *testing.T, w string) { writeTestFile(t, filepath.Join(w, "dir/a.txt"), "a\n") },
			want: []string{"Create dir/a.txt (copy of a.txt)"},
		},
		{
			name: "move", setup: map[string]string{"old.go": "package x\n"}, command: "mv old.go new.go",
			run: func(t *testing.T, w string) {
				if err := os.Rename(filepath.Join(w, "old.go"), filepath.Join(w, "new.go")); err != nil {
					t.Fatal(err)
				}
			},
			want: []string{"Move old.go -> new.go"},
		},
		{
			name: "glob delete", setup: map[string]string{"a.tmp": "1\n", "b.tmp": "2\n", "c.txt": "3\n"}, command: "rm *.tmp",
			run: func(t *testing.T, w string) {
				_ = os.Remove(filepath.Join(w, "a.tmp"))
				_ = os.Remove(filepath.Join(w, "b.tmp"))
			},
			want: []string{"Delete a.tmp", "Delete b.tmp"},
		},
		{
			name: "recursive delete", setup: map[string]string{"build/x.o": "x", "build/sub/y.o": "y"}, command: "rm -rf build",
			run:  func(t *testing.T, w string) { _ = os.RemoveAll(filepath.Join(w, "build")) },
			want: []string{"Delete build/sub/y.o", "Delete build/x.o"},
		},
		{
			name: "redirect", command: "cd sub && printf 'x\\n' > out.txt",
			setup: map[string]string{"sub/.keep": ""},
			run:   func(t *testing.T, w string) { writeTestFile(t, filepath.Join(w, "sub/out.txt"), "x\n") },
			want:  []string{"Create sub/out.txt"},
		},
		{
			name: "binary", setup: map[string]string{"blob": "\xff\x00"}, command: "truncate -s 0 blob",
			run:  func(t *testing.T, w string) { writeTestFile(t, filepath.Join(w, "blob"), "\xfe\x01") },
			want: []string{"Edit blob binary"},
		},
		{
			name: "symlink", command: "ln -s target link",
			run: func(t *testing.T, w string) {
				if err := os.Symlink("target", filepath.Join(w, "link")); err != nil {
					t.Fatal(err)
				}
			},
			want: []string{"Create link"},
		},
		{
			name: "unchanged", setup: map[string]string{"a": "same\n"}, command: "touch a",
			run: func(*testing.T, string) {},
		},
	} {
		t.Run(test.name, func(t *testing.T) {
			workspace := t.TempDir()
			for path, content := range test.setup {
				writeTestFile(t, filepath.Join(workspace, path), content)
			}
			observation := observeTestCommand(t, workspace, test.command)
			test.run(t, workspace)
			result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{})
			reviews, complete := testReviewFiles(t, result.Files), result.Complete
			if !complete {
				t.Fatalf("incomplete evidence: %+v", reviews)
			}
			if got := reviewSummary(workspace, reviews); !slices.Equal(got, test.want) {
				t.Fatalf("reviews = %q, want %q", got, test.want)
			}
		})
	}
}

// Running the commands checks capture against real shell semantics, where an
// earlier statement decides what a later destination names.
func TestExecObservationFollowsShellSemantics(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("bash is unavailable")
	}
	for _, test := range []struct {
		name    string
		setup   map[string]string
		links   map[string]string
		command string
		want    []string
		partial bool
	}{
		{name: "move into a new directory", setup: map[string]string{"a.txt": "a\n"}, command: "mkdir old && mv a.txt old", want: []string{"Move a.txt -> old/a.txt"}},
		{name: "copy into a new directory", setup: map[string]string{"app": "bin\n"}, command: "mkdir -p dist && cp app dist", want: []string{"Create dist/app (copy of app)"}},
		{
			name: "replace a directory", setup: map[string]string{"build/new.js": "new\n", "dist/old.js": "old\n"},
			command: "rm -rf dist && cp -r build dist", want: []string{"Create dist/new.js (copy of build/new.js)", "Delete dist/old.js"},
		},
		{
			name: "copy a tree created by the command", setup: map[string]string{"keep": ""},
			command: "mkdir src && printf 'x\\n' > src/new && cp -r src dst", want: []string{"Create dst/new", "Create src/new"},
		},
		{name: "glob over a created file", setup: map[string]string{"a.txt": "a\n", "dir/.keep": ""}, command: "printf 'b\\n' > b.txt && mv *.txt dir", want: []string{"Create dir/b.txt", "Move a.txt -> dir/a.txt"}},
		{name: "write through a symlink", setup: map[string]string{"real": "old\n"}, links: map[string]string{"link": "real"}, command: "printf 'new\\n' > link", want: []string{"Edit real"}},
		{name: "dangling symlink", links: map[string]string{"link": "missing"}, command: "printf 'new\\n' > link", want: []string{"Edit link incomplete"}, partial: true},
		{name: "empty files are not moves", setup: map[string]string{"empty": ""}, command: "rm empty && touch other", want: []string{"Create other", "Delete empty"}},
		{name: "NUL content is binary", setup: map[string]string{"data": "a\x00b"}, command: "printf 'c' >> data", want: []string{"Edit data binary"}},
	} {
		t.Run(test.name, func(t *testing.T) {
			workspace := t.TempDir()
			for path, content := range test.setup {
				writeTestFile(t, filepath.Join(workspace, path), content)
			}
			for link, target := range test.links {
				if err := os.Symlink(target, filepath.Join(workspace, link)); err != nil {
					t.Fatal(err)
				}
			}
			observation := observeTestCommand(t, workspace, test.command)
			command := exec.Command(bash, "-c", test.command)
			command.Dir = workspace
			if output, err := command.CombinedOutput(); err != nil {
				t.Fatalf("%s: %v\n%s", test.command, err, output)
			}
			result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{})
			reviews, complete := testReviewFiles(t, result.Files), result.Complete
			if complete == test.partial {
				t.Fatalf("complete = %v: %+v", complete, reviews)
			}
			if got := reviewSummary(workspace, reviews); !slices.Equal(got, test.want) {
				t.Fatalf("reviews = %q, want %q", got, test.want)
			}
		})
	}
}

func TestExecObservationSymlinkReviewsLinkTarget(t *testing.T) {
	workspace := t.TempDir()
	writeTestFile(t, filepath.Join(workspace, "target"), "secret content\n")
	observation := observeTestCommand(t, workspace, "ln -s target link")
	if err := os.Symlink("target", filepath.Join(workspace, "link")); err != nil {
		t.Fatal(err)
	}
	result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{})
	reviews := result.Files
	if len(reviews) != 1 || !strings.Contains(reviews[0].Diff, "+-> target") || strings.Contains(reviews[0].Diff, "secret") {
		t.Fatalf("symlink review = %+v", reviews)
	}
}

func TestExecObservationBoundsFileBytes(t *testing.T) {
	workspace := t.TempDir()
	large := filepath.Join(workspace, "large")
	writeTestFile(t, large, strings.Repeat("x", changecapture.MaxFileBytes+1))
	observation := observeTestCommand(t, workspace, "touch large")
	if result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{}); !result.Complete || len(result.Files) != 0 {
		t.Fatalf("an unchanged oversized file was reported: %+v", result.Files)
	}
	writeTestFile(t, large, strings.Repeat("y", changecapture.MaxFileBytes+2))
	if result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{}); result.Complete || len(result.Files) != 1 || result.Files[0].Incomplete == "" {
		t.Fatalf("a changed oversized file = %+v, complete=%v", result.Files, result.Complete)
	}
}

func TestExecObservationSkipsNeutralCommands(t *testing.T) {
	for _, command := range []string{"ls", "git status", `rg -n pattern internal/{router,ui}`, `find "$HOME/.codex/sessions" -type f -print`, `[ -d "$directory" ] && echo directory`, "skills-mgr info", "skills-mgr get golang-best-practices"} {
		for _, codeMode := range []bool{false, true} {
			if _, observed := changecapture.Capture(t.Context(), []commandanalysis.Command{{Command: command, Workdir: t.TempDir(), Shell: "bash"}}, false, codeMode, changecapture.Options{}); observed {
				t.Errorf("%q was observed without a declared scope", command)
			}
		}
	}
	if _, observed := changecapture.Capture(t.Context(), []commandanalysis.Command{{Command: "rm a", Workdir: t.TempDir(), Shell: "bash"}}, true, true, changecapture.Options{}); !observed {
		t.Error("a dynamic exec cell was not observed")
	}
}

func nativeExecOutput(state string) string {
	return "Chunk ID: 1\nWall time: 0.1 seconds\n" + state + "\nOutput:\n"
}

func TestLiveDiffTeeHeredocPreview(t *testing.T) {
	workspace := t.TempDir()
	writeTestFile(t, filepath.Join(workspace, "log"), "one\n")
	command := "tee -a log <<'EOF' >/dev/null\ntwo\nEOF\n"
	files, _, recognized, err := projection.Shell(t.Context(), command, workspace, true)
	if !recognized || err != nil {
		t.Fatalf("heredoc recognized=%v err=%v", recognized, err)
	}
	if got := reviewSummary(workspace, files); !slices.Equal(got, []string{"Edit log"}) {
		t.Fatalf("heredoc preview = %q", got)
	}
}

func TestLiveDiffFileOperationsPreserveSiblingStreams(t *testing.T) {
	workspace := t.TempDir()
	writeTestFile(t, filepath.Join(workspace, "a.txt"), "a\n")
	previewContext := t.Context()
	previewProject := previewProjector(previewContext, nativeExecCommandToolName)
	for _, test := range []struct {
		command string
		want    []string
	}{
		{"mv a.txt b.txt\n", nil},
		{"cp a.txt b.txt\n", nil},
		{"rm a.txt\n", nil},
		{"cp a.txt b.txt\ntee sibling.txt <<'EOF'\nnew content\nEOF\n", []string{"Create sibling.txt"}},
		{"rm a.txt\ncat > sibling.txt <<'EOF'\nnew content\nEOF\n", []string{"Create sibling.txt"}},
		{"mv a.txt b.txt\ncat > sibling.txt <<'EOF'\nnew content\nEOF\n", []string{"Create sibling.txt"}},
		{"cat > sibling.txt <<'EOF'\nnew content\nEOF\nmv a.txt b.txt\n", []string{"Create sibling.txt"}},
	} {
		preview, _ := previewProject(string(responses.MustMarshalJSON(map[string]string{"cmd": test.command})), workspace, false)
		if got := reviewSummary(workspace, preview.Files); !slices.Equal(got, test.want) {
			t.Fatalf("%q stream = %q, want %q", test.command, got, test.want)
		}
	}
	files, _, _, err := projection.Shell(previewContext, "mv a.txt b.txt\ncat > b.txt <<'EOF'\nnew content\nEOF\n", workspace, false)
	if len(files) != 0 || err == nil {
		t.Fatal("a dependent write borrowed the pre-move baseline")
	}
}

func TestExecReceiptAndSummaryText(t *testing.T) {
	workspace := t.TempDir()
	history := mekugiHistory{
		ExecOutcome: &changecapture.Outcome{Status: changecapture.StatusCompleted, Labels: []string{"cp"}},
		ReviewFiles: []diffmodel.ReviewFile{
			func() diffmodel.ReviewFile {
				file := diffmodel.RenderReviewFile("", filepath.Join(workspace, "b.txt"), "", "a\n")
				file.CopyFrom = filepath.Join(workspace, "a.txt")
				return file
			}(),
			diffmodel.RenderBinaryReviewFile(filepath.Join(workspace, "blob"), filepath.Join(workspace, "blob"), 2, 2, "aa", "bb"),
		},
	}
	want := "Create `b.txt` (copy of `a.txt`) +1 -0 · cp\n```diff\n@@ -0,0 +1,1 @@\n+a\n```\n\nEdit `blob` binary · cp"
	if got := changepresentation.ReceiptText(workspace, receiptEvidence(history)); got != want {
		t.Fatalf("receipt = %q, want %q", got, want)
	}
}
