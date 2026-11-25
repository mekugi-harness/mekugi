package main

import (
	"bytes"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"text/template"
)

var markdownLinkPattern = regexp.MustCompile(`\[([^\]]+)\]\([^)]+\)`)

const (
	indexTemplate = "mekugi-tui/apps/mekugi/cmd/workspace/modules.md.tmpl"
	indexOutput   = "docs/reference/modules.md"
)

type indexData struct {
	Modules []indexModule
}

type indexModule struct {
	ID        string
	Directory string
	Summary   string
	Packages  []string
}

func moduleIndex(members []member, check bool) error {
	entries, err := collectModuleReferences(members)
	if err != nil {
		return err
	}
	data := indexData{Modules: entries}
	slices.SortFunc(data.Modules, func(a, b indexModule) int {
		return strings.Compare(a.ID, b.ID)
	})
	tmpl, err := template.ParseFiles(indexTemplate)
	if err != nil {
		return err
	}
	var output bytes.Buffer
	if err := tmpl.Execute(&output, data); err != nil {
		return err
	}
	if check {
		retained, err := os.ReadFile(indexOutput)
		if err != nil {
			return err
		}
		if !bytes.Equal(retained, output.Bytes()) {
			return fmt.Errorf("%s is stale; run just generate", indexOutput)
		}
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(indexOutput), 0755); err != nil {
		return err
	}
	return os.WriteFile(indexOutput, output.Bytes(), 0644)
}

func collectModuleReferences(members []member) ([]indexModule, error) {
	var entries []indexModule
	for _, m := range members {
		dir := filepath.ToSlash(m.directory)
		if !strings.HasPrefix(dir, "mekugi-tui/modules/") {
			continue
		}
		id := filepath.Base(dir)
		summary := readSpecSummary(filepath.Join(m.directory, "SPEC.md"))
		pkgs, err := modulePackages(m.directory)
		if err != nil {
			return nil, err
		}
		entries = append(entries, indexModule{
			ID:        id,
			Directory: dir,
			Summary:   summary,
			Packages:  pkgs,
		})
	}
	return entries, nil
}

func readSpecSummary(path string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	var lines []string
	for line := range strings.SplitSeq(string(data), "\n") {
		trimmed := strings.TrimSpace(line)
		if trimmed == "" {
			if len(lines) > 0 {
				break
			}
			continue
		}
		if strings.HasPrefix(trimmed, "#") {
			continue
		}
		lines = append(lines, trimmed)
	}
	return markdownLinkPattern.ReplaceAllString(strings.Join(lines, " "), "$1")
}

func modulePackages(root string) ([]string, error) {
	seen := make(map[string]bool)
	err := filepath.WalkDir(root, func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			if path != root && (strings.HasPrefix(entry.Name(), ".") || entry.Name() == "testdata") {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		rel, err := filepath.Rel(root, filepath.Dir(path))
		if err != nil {
			return err
		}
		seen[filepath.ToSlash(rel)] = true
		return nil
	})
	if err != nil {
		return nil, err
	}
	pkgs := make([]string, 0, len(seen))
	for pkg := range seen {
		pkgs = append(pkgs, pkg)
	}
	slices.Sort(pkgs)
	return pkgs, nil
}
