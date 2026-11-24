package executionobservation

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
)

func TestExecBackupStarRetainsOperandDirectories(t *testing.T) {
	if exec.Command("sed", "--version").Run() != nil {
		t.Skip("BSD sed appends a backup suffix literally, without * expansion")
	}
	root := t.TempDir()
	writeTestFile(t, filepath.Join(root, "src/a.txt"), "old\n")
	if err := os.MkdirAll(filepath.Join(root, "backup/src"), 0700); err != nil {
		t.Fatal(err)
	}
	command := "sed -i'backup/*' 's/old/new/' src/*.txt"
	execProducerCommand(t, "sed")
	observation := execProducerCapture(t, root, command)
	execProducerRun(t, execProducerCommand(t, "bash"), root, command)
	result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{})
	reviews, complete, coverage := testReviewFiles(t, result.Files), result.Complete, result.Coverage
	if !complete || coverage != changecapture.CoverageExact {
		t.Fatalf("backup coverage: %v %s", complete, coverage)
	}
	if !slices.ContainsFunc(reviews, func(file diffmodel.ReviewFile) bool { return file.AfterPath == filepath.Join(root, "backup/src/a.txt") }) {
		t.Fatalf("backup absent: %+v", reviews)
	}
}

func TestExecInPlaceBackupGlobReconcilesExactBackupReview(t *testing.T) {
	t.Parallel()
	for _, tc := range []struct {
		name, binary, arguments string
	}{
		{name: "sed", binary: "sed", arguments: "-i.bak 's/old/new/' *.txt"},
		{name: "perl", binary: "perl", arguments: "-i.bak -pe 's/old/new/' *.txt"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			writer := execProducerCommand(t, tc.binary)
			bash := execProducerCommand(t, "bash")
			root := t.TempDir()
			writeTestFile(t, filepath.Join(root, "a.txt"), "old\n")

			command := writer + " " + tc.arguments
			observation := execProducerCapture(t, root, command)
			execProducerRun(t, bash, root, command)
			for path, want := range map[string]string{
				"a.txt":     "new\n",
				"a.txt.bak": "old\n",
			} {
				if got, err := os.ReadFile(filepath.Join(root, path)); err != nil || string(got) != want {
					t.Fatalf("host result %s = %q, %v; want %q", path, got, err, want)
				}
			}

			result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{})

			reviews, complete, coverage := testReviewFiles(t, result.Files), result.Complete, result.Coverage
			if !complete || coverage != changecapture.CoverageExact {
				t.Fatalf("reconciliation complete=%v coverage=%q; want exact evidence (reviews=%+v)", complete, coverage, reviews)
			}
			got := make(map[string]struct {
				fileTitle, diff, incomplete, origin string
			})
			for _, review := range reviews {
				path := review.AfterPath
				if path == "" {
					path = review.BeforePath
				}
				relative, err := filepath.Rel(root, path)
				if err != nil {
					t.Fatalf("review path %q outside root %q: %v", path, root, err)
				}
				got[filepath.ToSlash(relative)] = struct {
					fileTitle, diff, incomplete, origin string
				}{review.Action().Title(), review.Diff, review.Incomplete, review.Origin}
			}
			for relative, want := range map[string]struct {
				title, removed, added string
			}{
				"a.txt":     {title: "Edit", removed: "-old", added: "+new"},
				"a.txt.bak": {title: "Create", added: "+old"},
			} {
				review, ok := got[relative]
				if !ok || review.fileTitle != want.title || review.incomplete != "" || review.origin != "" ||
					!strings.Contains(review.diff, want.added) || want.removed != "" && !strings.Contains(review.diff, want.removed) {
					t.Errorf("review %s = %+v, want complete %s with %q %q", relative, review, want.title, want.removed, want.added)
				}
			}
			gotPaths := make([]string, 0, len(got))
			for path := range got {
				gotPaths = append(gotPaths, path)
			}
			slices.Sort(gotPaths)
			if !slices.Equal(gotPaths, []string{"a.txt", "a.txt.bak"}) {
				t.Errorf("review paths = %q, want [a.txt a.txt.bak]", gotPaths)
			}
		})
	}
}

func TestExecLnSfnCapturesReplacedSymlinkNotChild(t *testing.T) {
	t.Parallel()
	ln := execProducerCommand(t, "ln")
	bash := execProducerCommand(t, "bash")
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, "targetdir"), 0o700); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(root, "link")
	if err := os.Symlink("targetdir", link); err != nil {
		t.Fatal(err)
	}
	command := ln + " -sfn new link"
	observation := execProducerCapture(t, root, command)
	execProducerRun(t, bash, root, command)
	if target, err := os.Readlink(link); err != nil || target != "new" {
		t.Fatalf("host result link target = %q, %v; want replacement link to new", target, err)
	}
	if info, err := os.Stat(filepath.Join(root, "targetdir")); err != nil || !info.IsDir() {
		t.Fatalf("target directory changed: info=%v err=%v", info, err)
	}

	result := changecapture.Reconcile(t.Context(), *observation, changecapture.Comparison{})

	reviews, complete, coverage := testReviewFiles(t, result.Files), result.Complete, result.Coverage
	if !complete || coverage != changecapture.CoverageExact {
		t.Fatalf("reconciliation complete=%v coverage=%q reviews=%+v", complete, coverage, reviews)
	}
	if len(reviews) != 1 {
		t.Fatalf("reviews = %+v, want only the replaced symlink", reviews)
	}
	review := reviews[0]
	if review.BeforePath != link || review.AfterPath != link || review.Action().Title() != "Edit" || review.Incomplete != "" ||
		!strings.Contains(review.Diff, "targetdir") || !strings.Contains(review.Diff, "new") {
		t.Fatalf("review = %+v, want an exact edit of link from targetdir to new", review)
	}
	if child := filepath.Join(link, "new"); slices.Contains(observation.ScopePaths(), child) {
		t.Errorf("pre-call scope captured phantom child %q instead of the destination link", child)
	}
}
