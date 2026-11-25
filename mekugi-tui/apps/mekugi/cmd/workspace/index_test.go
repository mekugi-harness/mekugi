package main

import (
	"os"
	"strings"
	"testing"
)

func TestModuleIndexWriteCheckAndDrift(t *testing.T) {
	workspaceFixture(t, []string{"./mekugi-tui/modules/second", "./mekugi-tui/modules/first"}, map[string]string{
		"mekugi-tui/modules/first":  prefix + "/mekugi-tui/modules/first",
		"mekugi-tui/modules/second": prefix + "/mekugi-tui/modules/second",
	})
	for _, id := range []string{"first", "second"} {
		writeWorkspaceSource(t, "mekugi-tui/modules/"+id+"/SPEC.md", "# "+id+"\n\nSummary for "+id+".\n")
		writeWorkspaceSource(t, "mekugi-tui/modules/"+id+"/presentation/model.go", "package presentation\n")
	}
	writeWorkspaceSource(t, indexTemplate, "{{len .Modules}}{{range .Modules}} {{.ID}}:{{.Summary}}:{{range .Packages}}{{.}}{{end}}{{end}}")
	members, err := workspaceMembers()
	if err != nil {
		t.Fatal(err)
	}
	if err := moduleIndex(members, false); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(indexOutput)
	if err != nil {
		t.Fatal(err)
	}
	if want := "2 first:Summary for first.:presentation second:Summary for second.:presentation"; string(got) != want {
		t.Fatalf("index=%q, want %q", got, want)
	}
	if err := moduleIndex(members, true); err != nil {
		t.Fatalf("unchanged generation: %v", err)
	}
	writeWorkspaceSource(t, indexOutput, "stale")
	if err := moduleIndex(members, true); err == nil || !strings.Contains(err.Error(), "is stale") {
		t.Fatalf("drift check=%v", err)
	}
	if unchanged, err := os.ReadFile(indexOutput); err != nil || string(unchanged) != "stale" {
		t.Fatalf("check modified retained output: %q, %v", unchanged, err)
	}
	if err := moduleIndex(members, false); err != nil {
		t.Fatal(err)
	}
	if regenerated, err := os.ReadFile(indexOutput); err != nil || string(regenerated) != string(got) {
		t.Fatalf("regeneration=%q, %v; want %q", regenerated, err, got)
	}
	writeWorkspaceSource(t, "go.work", "go 1.27.0\nuse (\n./mekugi-tui/modules/first\n)\n")
	members, err = workspaceMembers()
	if err != nil {
		t.Fatal(err)
	}
	if err := moduleIndex(members, false); err != nil {
		t.Fatal(err)
	}
	if got, err := os.ReadFile(indexOutput); err != nil || string(got) != "1 first:Summary for first.:presentation" {
		t.Fatalf("workspace removal was not reflected: %q, %v", got, err)
	}
}
