package main

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func TestWorkspaceSnapshotPackageDiscovery(t *testing.T) {
	workspaceFixture(t, []string{"./mekugi-tui/libraries/first"}, map[string]string{"mekugi-tui/libraries/first": prefix + "/mekugi-tui/libraries/first"})
	for path, source := range map[string]string{
		"render/snapshot_test.go": "package render_test\nimport \"testing\"\nfunc TestUISnapshotFrame(t *testing.T) {}\nfunc TestUISnapshotOther(t *testing.T) {}\n",
		"render/_ignored_test.go": "package render\nfunc TestUISnapshotIgnored(\n",
		"render/.ignored_test.go": "package render\nfunc TestUISnapshotIgnored(\n",
		"harness/check_test.go":   "package harness\nimport \"testing\"\nfunc TestSnapshotComparison(t *testing.T) {}\n",
		"tagged/snapshot_test.go": "//go:build ignore\n\npackage tagged\nfunc TestUISnapshotPlatform() {}\n",
		"plain/normal_test.go":    "package plain\n// TestUISnapshotComment\nfunc TestNormal() {}\nfunc (T) TestUISnapshotMethod() {}\n",
		"plain/source.go":         "package plain\nfunc TestUISnapshotNotATestFile() {}\n",
		"testdata/skip_test.go":   "package skipped\nfunc TestUISnapshotIgnored() {}\n",
		"vendor/skip_test.go":     "package skipped\nfunc TestUISnapshotIgnored() {}\n",
		".hidden/skip_test.go":    "package skipped\nfunc TestUISnapshotIgnored() {}\n",
		"_hidden/skip_test.go":    "package skipped\nfunc TestUISnapshotIgnored() {}\n",
		"nested/go.mod":           "module nested\n",
		"nested/skip_test.go":     "package nested\nfunc TestUISnapshotIgnored() {}\n",
	} {
		writeWorkspaceSource(t, filepath.Join("mekugi-tui/libraries/first", path), source)
	}
	members, err := workspaceMembers()
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"./mekugi-tui/libraries/first/harness/...", "./mekugi-tui/libraries/first/render/...", "./mekugi-tui/libraries/first/tagged/..."}
	got, err := expand([]string{"@snapshots"}, members)
	if err != nil || !slices.Equal(got, want) {
		t.Fatalf("snapshot packages=%v error=%v, want %v", got, err, want)
	}
	// Only snapshot packages are compiled: plain contains invalid ordinary Go
	// code, and tagged has no files runnable on this platform.
	if err := run([]string{"run", "go", "test", "@snapshots", "-run", "TestSnapshot|TestUISnapshot", "-count=1"}); err != nil {
		t.Fatalf("run discovered snapshot packages: %v", err)
	}
	writeWorkspaceSource(t, "mekugi-tui/libraries/first/render/snapshot_test.go", "package render\nfunc TestUISnapshotBroken(\n")
	if _, err := expand([]string{"@snapshots"}, members); err == nil {
		t.Fatal("malformed snapshot declaration was ignored")
	}
}

func TestWorkspaceSnapshotCoverageRejectsAnUnusedFileInATestedPackage(t *testing.T) {
	workspaceFixture(t, []string{"./mekugi-tui/libraries/first"}, map[string]string{"mekugi-tui/libraries/first": prefix + "/mekugi-tui/libraries/first"})
	const pkg = "./mekugi-tui/libraries/first/render"
	good := filepath.Join(pkg, "testdata/snapshots/current.txt")
	orphan := filepath.Join(pkg, "testdata/snapshots/unused.txt")
	writeWorkspaceSource(t, good, "current\n")
	writeWorkspaceSource(t, orphan, "unused\n")
	visits := t.TempDir()
	path, err := filepath.Abs(good)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(visits, "visit-test"), []byte(path), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := validateSnapshotVisits(visits, []string{pkg + "/..."}); err == nil || !strings.Contains(err.Error(), orphan) || strings.Contains(err.Error(), good) {
		t.Fatalf("expected only the unused baseline, got %v", err)
	}
}

func TestWorkspaceSnapshotPackageDiscoveryRejectsEmpty(t *testing.T) {
	workspaceFixture(t, []string{"./mekugi-tui/libraries/first"}, map[string]string{"mekugi-tui/libraries/first": prefix + "/mekugi-tui/libraries/first"})
	if err := run([]string{"run", "go", "test", "@snapshots"}); err == nil || !strings.Contains(err.Error(), "no snapshot test packages") {
		t.Fatalf("empty selection error=%v", err)
	}
}

func TestWorkspaceSnapshotPackageDiscoveryRejectsOrphans(t *testing.T) {
	workspaceFixture(t, []string{"./mekugi-tui/libraries/first"}, map[string]string{"mekugi-tui/libraries/first": prefix + "/mekugi-tui/libraries/first"})
	writeWorkspaceSource(t, "mekugi-tui/libraries/first/render/snapshot_test.go", "package render\nimport \"testing\"\nfunc TestUISnapshotFrame(t *testing.T) {}\n")
	writeWorkspaceSource(t, "mekugi-tui/libraries/first/orphan/plain_test.go", "package orphan\nimport \"testing\"\nfunc TestPlain(t *testing.T) {}\n")
	writeWorkspaceSource(t, "mekugi-tui/libraries/first/orphan/testdata/snapshots/stale.txt", "stale baseline\n")
	members, err := workspaceMembers()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := expand([]string{"@snapshots"}, members); err == nil || !strings.Contains(err.Error(), "orphan/testdata/snapshots") {
		t.Fatalf("expected orphaned snapshot error, got %v", err)
	}
}
