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
		{"mekugi-tui/libraries/persistence", ".", "mekugi-tui/contracts/observation", false},
		{"mekugi-tui/libraries/terminalui", ".", "mekugi-tui/libraries/sourceanalysis/markdownsyntax", true},
		{"mekugi-tui/libraries/sourceanalysis", "markdownsyntax", "mekugi-tui/libraries/terminalui", false},
		{"mekugi-tui/libraries/diffrender", ".", "mekugi-tui/libraries/textsanitize", true},
		{"mekugi-tui/libraries/terminalui", ".", "mekugi-tui/libraries/textsanitize", true},
		{"mekugi-tui/libraries/textsanitize", ".", "mekugi-tui/libraries/diffrender", false},
		{"mekugi-tui/modules/activity", "presentation", "mekugi-tui/libraries/textsanitize", true},
		{"mekugi-tui/libraries/pathdisplay", ".", "mekugi-tui/libraries/persistence", false},
		{"mekugi-tui/backends/codex", "binding", "mekugi-tui/contracts/observation", true},
		{"mekugi-tui/backends/codex", "binding", "mekugi-tui/providers/openai", false},
		{"mekugi-tui/providers/openai", ".", "mekugi-tui/libraries/persistence", true},
		{"mekugi-tui/providers/openai", ".", "mekugi-tui/backends/codex/binding", false},
		{"mekugi-tui/contracts", "observation", "mekugi-tui/modules/reader", false},
		{"mekugi-tui/testsupport/uisnapshot", ".", "mekugi-tui/backends/codex/binding", true},
		{"mekugi-tui/apps/mekugi", "internal/composition", "mekugi-tui/testsupport/uisnapshot", false},
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
		{"mekugi-webui", ".", "mekugi-tui/apps/mekugi/internal/sessionapi", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-tui/apps/mekugi/internal/sessionpresentation", true},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-webui", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-tui/apps/mekugi/internal/terminalsession", false},
		{"mekugi-tui/apps/mekugi", "internal/sessionapi", "mekugi-tui/modules/activity/terminalview", false},
		{"mekugi-webui", ".", "mekugi-tui/apps/mekugi/internal/terminalsession", false},
		{"mekugi-webui", ".", "mekugi-tui/modules/activity/terminalview", false},
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
