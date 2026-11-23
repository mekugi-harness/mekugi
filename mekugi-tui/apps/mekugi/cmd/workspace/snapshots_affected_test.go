package main

import (
	"os"
	"slices"
	"strings"
	"testing"
)

func TestAffectedSnapshotPackages(t *testing.T) {
	t.Setenv("MEKUGI_STAGED_SNAPSHOT_CHANGED_PATHS", "changed")
	workspaceFixture(t, []string{"./mekugi-tui/libraries/render", "./mekugi-tui/apps/view", "./mekugi-tui/apps/mekugi"}, map[string]string{
		"mekugi-tui/libraries/render": prefix + "/mekugi-tui/libraries/render", "mekugi-tui/apps/view": prefix + "/mekugi-tui/apps/view",
		"mekugi-tui/apps/mekugi": prefix + "/mekugi-tui/apps/mekugi",
	})
	for path, source := range map[string]string{
		"mekugi-tui/libraries/render/shared/source.go": "package shared\n",
		"mekugi-tui/libraries/render/snapshot_test.go": "package render_test\nimport (\"testing\"; _ \"" + prefix + "/mekugi-tui/libraries/render/shared\")\nfunc TestUISnapshotRender(t *testing.T) {}\n",
		"mekugi-tui/apps/view/snapshot_test.go":        "package view_test\nimport (\"testing\"; _ \"" + prefix + "/mekugi-tui/libraries/render\")\nfunc TestUISnapshotView(t *testing.T) {}\n",
		"mekugi-tui/apps/view/view.go":                 "package view\n",
		"mekugi-tui/libraries/render/render.go":        "package render\n",
		// Include tagged and external test imports even on a different platform.
		"mekugi-tui/apps/view/platform_windows_test.go": "package view_test\nimport _ \"" + prefix + "/mekugi-tui/libraries/render/tagged\"\n",
		"mekugi-tui/libraries/render/tagged/tagged.go":  "package tagged\n",
		"mekugi-tui/libraries/render/other/other.go":    "package other\n",
	} {
		writeWorkspaceSource(t, path, source)
	}
	members, err := workspaceMembers()
	if err != nil {
		t.Fatal(err)
	}
	all := []string{"./mekugi-tui/apps/view/...", "./mekugi-tui/libraries/render/..."}
	for _, tc := range []struct {
		name  string
		paths []string
		want  []string
	}{
		{"shared source", []string{"mekugi-tui/libraries/render/shared/source.go"}, all},
		{"deleted source", []string{"mekugi-tui/libraries/render/shared/deleted.go"}, all},
		{"external tagged test import", []string{"mekugi-tui/libraries/render/tagged/tagged.go"}, all},
		{"application only", []string{"mekugi-tui/apps/view/view.go"}, all[:1]},
		{"fixture only", []string{"mekugi-tui/libraries/render/testdata/snapshots/a name\nwith newline.txt"}, all[1:]},
		{"fixture and source", []string{"mekugi-tui/libraries/render/testdata/snapshots/a.txt", "mekugi-tui/apps/view/view.go"}, all},
		{"new or renamed source", []string{"mekugi-tui/apps/view/old.go", "mekugi-tui/apps/view/new.go"}, all[:1]},
		{"module dependency", []string{"mekugi-tui/libraries/render/go.mod"}, all},
		{"workspace dependency", []string{"go.work.sum"}, all},
		{"tooling", []string{".githooks/pre-commit"}, all},
		{"runner source", []string{"mekugi-tui/apps/mekugi/cmd/workspace/main.go"}, all},
		{"embedded resource", []string{"mekugi-tui/apps/view/assets/page.html"}, all},
		{"unknown fixture owner", []string{"mekugi-tui/libraries/render/shared/testdata/snapshots/a.txt"}, all},
		{"source fixture", []string{"mekugi-tui/apps/view/testdata/example.go"}, all},
		{"vendored source", []string{"mekugi-tui/apps/view/vendor/example.go"}, all},
		{"no changes", nil, all},
	} {
		t.Run(tc.name, func(t *testing.T) {
			changed := strings.Join(tc.paths, "\x00")
			if len(tc.paths) > 0 {
				changed += "\x00"
			}
			writeWorkspaceSource(t, "changed", changed)
			got, err := expand([]string{"@snapshots"}, members)
			if err != nil || !slices.Equal(got, tc.want) {
				t.Fatalf("packages=%v, err=%v; want %v", got, err, tc.want)
			}
		})
	}
	// Unknown ownership cannot narrow the gate.
	writeWorkspaceSource(t, "mekugi-tui/apps/viewtool/source.go", "package viewtool\n")
	if got, err := affectedSnapshotPackages(members, all, []byte("mekugi-tui/apps/viewtool/source.go\x00")); err != nil || !slices.Equal(got, all) {
		t.Fatalf("outside member must retain full gate: %v, %v", got, err)
	}
	writeWorkspaceSource(t, "changed", "mekugi-tui/apps/view/view.go")
	if _, err := expand([]string{"@snapshots"}, members); err == nil {
		t.Fatal("accepted incomplete changed-path list")
	}
	if err := os.Remove("changed"); err != nil {
		t.Fatal(err)
	}
	if _, err := expand([]string{"@snapshots"}, members); err == nil {
		t.Fatal("accepted missing changed-path list")
	}
	// Explicit packages do not consume the opt-in path, even if it is invalid.
	if got, err := expand(all, members); err != nil || !slices.Equal(got, all) {
		t.Fatalf("explicit selection=%v, err=%v", got, err)
	}
	t.Setenv("MEKUGI_STAGED_SNAPSHOT_CHANGED_PATHS", "")
	if got, err := expand([]string{"@snapshots"}, members); err != nil || !slices.Equal(got, all) {
		t.Fatalf("default selection=%v, err=%v; want %v", got, err, all)
	}
}

func TestAffectedSnapshotsExcludeUnrelatedPackages(t *testing.T) {
	t.Setenv("MEKUGI_STAGED_SNAPSHOT_CHANGED_PATHS", "changed")
	workspaceFixture(t, []string{"./mekugi-tui/libraries/render"}, map[string]string{"mekugi-tui/libraries/render": prefix + "/mekugi-tui/libraries/render"})
	writeWorkspaceSource(t, "mekugi-tui/libraries/render/view/snapshot_test.go", "package view\nimport \"testing\"\nfunc TestUISnapshotView(t *testing.T) {}\n")
	writeWorkspaceSource(t, "mekugi-tui/libraries/render/tool/source.go", "package tool\n")
	members, err := workspaceMembers()
	if err != nil {
		t.Fatal(err)
	}
	writeWorkspaceSource(t, "changed", "mekugi-tui/libraries/render/tool/source.go\x00")
	got, err := expand([]string{"@snapshots"}, members)
	// The make recipe still runs the snapshot harness explicitly.
	if err != nil || len(got) != 0 {
		t.Fatalf("unrelated selection=%v, err=%v", got, err)
	}
}
