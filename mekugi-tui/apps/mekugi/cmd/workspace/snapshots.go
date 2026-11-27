package main

import (
	"bytes"
	"errors"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
)

// snapshotPackages finds snapshot declarations before go test compiles packages.
// Scan all platforms conservatively; Go still selects runnable files and tests.
func snapshotPackages(members []member) ([]string, error) {
	packages := map[string]bool{}
	baselineDirs := map[string]bool{}
	for _, m := range members {
		err := filepath.WalkDir(m.directory, func(path string, entry fs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if entry.IsDir() {
				if path != m.directory {
					if entry.Name() == "testdata" {
						snapDir := filepath.Join(path, "snapshots")
						if entries, err := os.ReadDir(snapDir); err == nil {
							for _, se := range entries {
								if !se.IsDir() && strings.HasSuffix(se.Name(), ".txt") {
									baselineDirs[filepath.ToSlash(filepath.Dir(path))] = true
									break
								}
							}
						}
						return filepath.SkipDir
					}
					if strings.HasPrefix(entry.Name(), ".") || strings.HasPrefix(entry.Name(), "_") || entry.Name() == "vendor" || entry.Name() == "node_modules" || entry.Name() == "dist" {
						return filepath.SkipDir
					}
					if _, err := os.Stat(filepath.Join(path, "go.mod")); err == nil {
						return filepath.SkipDir
					}
				}
				return nil
			}
			if strings.HasPrefix(entry.Name(), ".") || strings.HasPrefix(entry.Name(), "_") || !strings.HasSuffix(path, "_test.go") {
				return nil
			}
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			if !bytes.Contains(data, []byte("TestUISnapshot")) && !bytes.Contains(data, []byte("TestSnapshot")) {
				return nil
			}
			file, err := parser.ParseFile(token.NewFileSet(), path, data, parser.SkipObjectResolution)
			if err != nil {
				return err
			}
			for _, decl := range file.Decls {
				if function, ok := decl.(*ast.FuncDecl); ok && function.Recv == nil && (strings.HasPrefix(function.Name.Name, "TestUISnapshot") || strings.HasPrefix(function.Name.Name, "TestSnapshot")) {
					// Patterns let Go skip directories containing only files for
					// another platform, just as the full workspace run does.
					packages["./"+filepath.ToSlash(filepath.Dir(path))+"/..."] = true
				}
			}
			return nil
		})
		if err != nil {
			return nil, err
		}
	}
	var orphans []string
	for dir := range baselineDirs {
		if !packages["./"+dir+"/..."] {
			orphans = append(orphans, dir+"/testdata/snapshots")
		}
	}
	if len(orphans) > 0 {
		slices.Sort(orphans)
		return nil, fmt.Errorf("orphaned UI snapshot baselines with no TestUISnapshot/TestSnapshot tests: %s", strings.Join(orphans, ", "))
	}
	if len(packages) == 0 {
		return nil, errors.New("workspace has no snapshot test packages")
	}
	result := make([]string, 0, len(packages))
	for path := range packages {
		result = append(result, path)
	}
	slices.Sort(result)
	return result, nil
}

func checkSnapshotCoverage(command *exec.Cmd, packages []string) error {
	visits, err := os.MkdirTemp("", "mekugi-snapshot-visits-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(visits)
	command.Env = append(os.Environ(), "MEKUGI_SNAPSHOT_VISITS="+visits)
	if err := compactTests(command); err != nil {
		return err
	}
	return validateSnapshotVisits(visits, packages)
}

func validateSnapshotVisits(visits string, packages []string) error {
	used := map[string]bool{}
	entries, err := os.ReadDir(visits)
	if err != nil {
		return err
	}
	for _, entry := range entries {
		path, err := os.ReadFile(filepath.Join(visits, entry.Name()))
		if err != nil {
			return err
		}
		used[string(path)] = true
	}
	var orphans []string
	for _, pkg := range packages {
		directory, ok := strings.CutSuffix(pkg, "/...")
		if !ok || !strings.HasPrefix(directory, "./") {
			continue
		}
		paths, err := filepath.Glob(filepath.Join(directory, "testdata", "snapshots", "*.txt"))
		if err != nil {
			return err
		}
		for _, path := range paths {
			absolute, err := filepath.Abs(path)
			if err != nil {
				return err
			}
			if !used[absolute] {
				orphans = append(orphans, filepath.ToSlash(path))
			}
		}
	}
	if len(orphans) == 0 {
		return nil
	}
	slices.Sort(orphans)
	return fmt.Errorf("orphaned UI snapshot baselines not asserted by any test: %s", strings.Join(orphans, ", "))
}
