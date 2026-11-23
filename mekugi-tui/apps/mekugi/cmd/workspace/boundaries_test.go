package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeWorkspaceSource(t *testing.T, path, source string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(source), 0600); err != nil {
		t.Fatal(err)
	}
}

func TestWorkspaceImportBoundaries(t *testing.T) {
	for _, tt := range []struct {
		owner, pkg, target string
		allowed            bool
	}{
		{"mekugi-tui/libraries/legacycall", ".", "mekugi-tui/libraries/persistence", true},
		{"mekugi-tui/libraries/legacycall", ".", "mekugi-tui/contracts/recordkind", true},
		{"mekugi-tui/libraries/legacycall", ".", "mekugi-tui/contracts/command", false},
		{"mekugi-tui/libraries/persistence", ".", "mekugi-tui/contracts/recordkind", false},
		{"mekugi-tui/libraries/persistence", ".", "mekugi-tui/libraries/legacycall", false},
		{"mekugi-tui/libraries/outputbudget", ".", "mekugi-tui/libraries/sourceanalysis/rows", true},
		{"mekugi-tui/libraries/terminalui", ".", "mekugi-tui/libraries/sourceanalysis/markdownsyntax", true},
		{"mekugi-tui/libraries/sourceanalysis", "markdownsyntax", "mekugi-tui/libraries/terminalui", false},
		{"mekugi-tui/libraries/diffrender", ".", "mekugi-tui/libraries/textsanitize", true},
		{"mekugi-tui/libraries/terminalui", ".", "mekugi-tui/libraries/textsanitize", true},
		{"mekugi-tui/libraries/textsanitize", ".", "mekugi-tui/libraries/diffrender", false},
		{"mekugi-tui/modules/activity", "presentation", "mekugi-tui/libraries/textsanitize", true},
		{"mekugi-tui/libraries/outputbudget", ".", "mekugi-tui/backends/codex/binding", false},
		{"mekugi-tui/libraries/pathdisplay", ".", "mekugi-tui/libraries/persistence", false},
		{"mekugi-tui/backends/codex", "binding", "mekugi-tui/contracts/command", true},
		{"mekugi-tui/backends/codex", "binding", "mekugi-tui/providers/openai", false},
		{"mekugi-tui/providers/openai", ".", "mekugi-tui/libraries/persistence", true},
		{"mekugi-tui/providers/openai", ".", "mekugi-tui/backends/codex/binding", false},
		{"mekugi-tui/contracts", "command", "mekugi-tui/modules/reader", false},
		{"mekugi-tui/testsupport/moduletest", ".", "mekugi-tui/backends/codex/binding", true},
		{"mekugi-tui/apps/mekugi", "internal/composition", "mekugi-tui/testsupport/moduletest", false},
		{"mekugi-tui/apps/mekugi", "internal/composition", "mekugi-tui/apps/mekugi/internal/launch", true},
		{"mekugi-tui/apps/mekugi", "internal/sessioncontrol", "mekugi-tui/backends/codex/appserver", true},
		{"mekugi-tui/apps/mekugi", "internal/sessioncontrol", "mekugi-tui/apps/mekugi/internal/launch", false},
		{"mekugi-tui/apps/mekugi", "internal/sessioncontrol", "mekugi-tui/libraries/terminalui", false},
		{"mekugi-tui/apps/mekugi", "internal/sessioncontrol", "mekugi-tui/modules/activity/terminalview", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionpresentation", "mekugi-tui/backends/codex/appserver", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionpresentation", "mekugi-tui/modules/activity/presentation", true},
		{"mekugi-tui/apps/mekugi", "internal/sessionpresentation/model", "mekugi-tui/apps/mekugi/internal/terminalsession", false},
		{"mekugi-tui/apps/mekugi", "internal/headless", "mekugi-tui/apps/mekugi/internal/sessioncontrol", true},
		{"mekugi-tui/apps/mekugi", "internal/headless", "mekugi-tui/apps/mekugi/internal/sessionpresentation", false},
		{"mekugi-tui/apps/mekugi", "internal/headless", "mekugi-tui/libraries/diffrender", false},
		{"mekugi-tui/apps/mekugi", "internal/inspectioncli", "mekugi-tui/apps/mekugi/internal/launch", false},
		{"mekugi-webui", ".", "mekugi-tui/apps/mekugi/internal/sessionapi", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-tui/apps/mekugi/internal/sessionpresentation", true},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-webui", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-tui/apps/mekugi/internal/terminalsession", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-tui/modules/activity/terminalview", false},
		{"mekugi-webui", ".", "mekugi-tui/apps/mekugi/internal/terminalsession", false},
		{"mekugi-webui", ".", "mekugi-tui/modules/activity/terminalview", false},
		{"mekugi-tui/apps/mekugi", "internal/inspectioncli", "mekugi-tui/apps/mekugi/internal/inferencegateway", false},
		{"mekugi-tui/apps/mekugi", "internal/inspectioncli", "mekugi-tui/libraries/terminalui", false},
		{"mekugi-tui/apps/mekugi", "internal/inspectioncli", "mekugi-tui/modules/activity/terminalview", false},
		{"mekugi-tui/apps/mekugi", "internal/inspectioncli", "mekugi-tui/modules/sessionhistory/history", true},
		{"mekugi-tui/apps/mekugi", "internal/inspectioncli", "mekugi-tui/libraries/legacycall", true},
		{"mekugi-tui/apps/mekugi", "internal/frontends", "mekugi-tui/modules/fileread/read", true},
		{"mekugi-tui/apps/mekugi", "internal/frontends/worker", "mekugi-tui/apps/mekugi/internal/launch", false},
		{"mekugi-tui/apps/mekugi", "internal/frontends", "mekugi-tui/apps/mekugi/internal/composition", false},
		{"mekugi-tui/apps/mekugi", "internal/frontends", "mekugi-tui/apps/mekugi/internal/launch", false},
		{"mekugi-tui/modules/reader", "codexbinding", "mekugi-tui/backends/codex/binding", true},
		{"mekugi-tui/modules/reader", "internal", "mekugi-tui/backends/codex/binding", false},
		{"mekugi-tui/modules/reader", "terminalview", "mekugi-tui/libraries/terminalui", true},
		{"mekugi-tui/modules/reader", "presentation", "mekugi-tui/libraries/terminalui", false},
		{"mekugi-tui/modules/reader", "presentation", "mekugi-tui/modules/reader/terminalview", false},
		{"mekugi-tui/modules/reader", "internal", "mekugi-tui/modules/reader/codexbinding", false},
	} {
		t.Run(tt.owner+"/"+tt.pkg+"/"+tt.target, func(t *testing.T) {
			target := prefix + "/" + tt.target
			if err := allowedImport(tt.owner, tt.pkg, target); (err == nil) != tt.allowed {
				t.Fatalf("allowedImport error=%v want allowed=%v", err, tt.allowed)
			}
			workspaceFixture(t, []string{"./" + tt.owner}, map[string]string{tt.owner: prefix + "/" + tt.owner})
			if strings.HasPrefix(tt.owner, "mekugi-tui/modules/") {
				writeWorkspaceSource(t, filepath.Join(tt.owner, "module.go"), `package reader
import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
type Deps struct{}
var Module = module.Descriptor[Deps, int, int, int]{}
`)
			}
			path := filepath.Join(tt.owner, tt.pkg, "source.go")
			writeWorkspaceSource(t, path, fmt.Sprintf("package fixture\nimport _ %q\n", target))
			err := checkImports([]member{{tt.owner, prefix + "/" + tt.owner}})
			if (err == nil) != tt.allowed {
				t.Fatalf("checkImports error=%v want allowed=%v", err, tt.allowed)
			}
			if !tt.allowed && (!strings.Contains(err.Error(), path) || !strings.Contains(err.Error(), target)) {
				t.Fatalf("failure lacks source/import identity: %v", err)
			}
		})
	}
}

func TestRendererNeutralApplicationImports(t *testing.T) {
	for _, pkg := range []string{"internal/sessioncontrol", "internal/sessionpresentation", "internal/headless", "internal/sessionapi"} {
		for _, target := range []string{"github.com/charmbracelet/ultraviolet", "golang.org/x/term"} {
			t.Run(pkg+"/"+target, func(t *testing.T) {
				if err := allowedImport("mekugi-tui/apps/mekugi", pkg, target); err == nil {
					t.Fatalf("renderer-neutral %s imported %s", pkg, target)
				}
			})
		}
	}
}

func TestAllowedModulePublicAndRendererImports(t *testing.T) {
	for _, tt := range []struct {
		pkg, target string
		allowed     bool
	}{
		{"internal", "mekugi-tui/modules/history/publication", true},
		{"internal", "mekugi-tui/modules/history", false},
		{"internal", "mekugi-tui/modules/history/internal/records", false},
		{"internal", "mekugi-tui/modules/history/codexbinding", false},
		{"internal", "mekugi-tui/modules/history/terminalview", false},
		{"internal", "mekugi-tui/modules/history/presentation", false},
		{"codexbinding", "mekugi-tui/backends/codex/internal", false},
		{"presentation", "github.com/charmbracelet/lipgloss", false},
		{"terminalview", "github.com/charmbracelet/lipgloss", true},
		{"presentation", "golang.org/x/term", false},
	} {
		t.Run(tt.pkg+"/"+tt.target, func(t *testing.T) {
			target := tt.target
			if !strings.Contains(target, ".") {
				target = prefix + "/" + target
			}
			if err := allowedImport("mekugi-tui/modules/reader", tt.pkg, target); (err == nil) != tt.allowed {
				t.Fatalf("error=%v want allowed=%v", err, tt.allowed)
			}
		})
	}
}

func TestWorkspaceChecksAllProductionPlatformsButNotTests(t *testing.T) {
	const owner = "mekugi-tui/libraries/pathdisplay"
	workspaceFixture(t, []string{"./" + owner}, map[string]string{owner: prefix + "/" + owner})
	forbidden := prefix + "/mekugi-tui/backends/codex/binding"
	source := fmt.Sprintf("package fixture\nimport _ %q\n", forbidden)
	writeWorkspaceSource(t, filepath.Join(owner, "source_test.go"), source)
	if err := checkImports([]member{{owner, prefix + "/" + owner}}); err != nil {
		t.Fatalf("test-only import rejected: %v", err)
	}
	tagged := filepath.Join(owner, "source_plan9.go")
	writeWorkspaceSource(t, tagged, "//go:build plan9\n\n"+source)
	if err := checkImports([]member{{owner, prefix + "/" + owner}}); err == nil || !strings.Contains(err.Error(), tagged) {
		t.Fatalf("platform-specific production import escaped: %v", err)
	}
}

func TestModuleRequiresMatchesImportsAndTypedDeps(t *testing.T) {
	const owner = "mekugi-tui/modules/reader"
	const template = `package reader
import (
 "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
 retained "github.com/mekugi-harness/mekugi/mekugi-tui/modules/history/publication"
)
type Deps struct{%s}
var Module = module.Descriptor[Deps, int, int, int]{Requires: %s}
`
	for _, tt := range []struct{ name, deps, requires, extra, reason string }{
		{"matching", "History retained.Publisher", `[]module.ID{"history"}`, "", ""},
		{"missing requirement", "History retained.Publisher", `[]module.ID{}`, "", "must match"},
		{"pure module API", "", `[]module.ID{"history"}`, "", ""},
		{"extra requirement", "History retained.Publisher", `[]module.ID{"history","extra"}`, "", "must match"},
		{"platform import", "History retained.Publisher", `[]module.ID{"history"}`, "platform", "must match"},
		{"test import ignored", "History retained.Publisher", `[]module.ID{"history"}`, "test", ""},
		{"repeated requirement", "History retained.Publisher", `[]module.ID{"history","history"}`, "", "repeated requirement"},
		{"computed requirements", "History retained.Publisher", `requiredModules`, "", "Requires must be a literal list"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			workspaceFixture(t, []string{"./" + owner}, map[string]string{owner: prefix + "/" + owner})
			writeWorkspaceSource(t, filepath.Join(owner, "module.go"), fmt.Sprintf(template, tt.deps, tt.requires))
			if tt.extra != "" {
				file := "extra_test.go"
				tag := ""
				if tt.extra == "platform" {
					file = "extra_plan9.go"
					tag = "//go:build plan9\n\n"
				}
				writeWorkspaceSource(t, filepath.Join(owner, "internal", file), tag+"package internal\nimport _ \"github.com/mekugi-harness/mekugi/mekugi-tui/modules/extra/api\"\n")
			}
			err := checkImports([]member{{owner, prefix + "/" + owner}})
			if tt.reason == "" {
				if err != nil {
					t.Fatal(err)
				}
			} else if err == nil || !strings.Contains(err.Error(), tt.reason) {
				t.Fatalf("error=%v, want %q", err, tt.reason)
			}
		})
	}
}
