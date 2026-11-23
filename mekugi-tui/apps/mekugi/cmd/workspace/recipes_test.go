package main

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// repositoryMakefile reads the repository Makefile through a testdata symlink.
// Go's test cache tracks only files inside this module, so reading the link
// keys cached results on the Makefile, including recipes make runs from it.
func repositoryMakefile(t *testing.T) []byte {
	t.Helper()
	makefile, err := os.ReadFile("testdata/Makefile")
	if err != nil {
		t.Fatal(err)
	}
	return makefile
}

func TestMakefileValidationOutput(t *testing.T) {
	makefile := repositoryMakefile(t)
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	workspaceFixture(t, []string{"./mekugi-tui/libraries/fixture", "./mekugi-tui/testsupport/uisnapshot"}, map[string]string{
		"mekugi-tui/libraries/fixture":      prefix + "/mekugi-tui/libraries/fixture",
		"mekugi-tui/testsupport/uisnapshot": prefix + "/mekugi-tui/testsupport/uisnapshot",
	})
	writeWorkspaceSource(t, "Makefile", string(makefile))
	writeWorkspaceSource(t, ".deadcode-allowlist", "")
	writeWorkspaceSource(t, "mekugi-tui/testsupport/uisnapshot/fixture.go", "package uisnapshot\n")
	writeWorkspaceSource(t, "mekugi-tui/libraries/fixture/empty/fixture.go", "package empty\n")
	writeWorkspaceSource(t, "mekugi-tui/libraries/fixture/fixture_test.go", `package fixture
import ("fmt"; "strings"; "testing")
func TestPassingNoise(t *testing.T) {
 for range 11000 { t.Log(strings.Repeat("x", 1024)) }
}
func TestPassingStdout(t *testing.T) {
 for range 11000 { fmt.Println(strings.Repeat("y", 1024)) }
}
func TestFailure(t *testing.T) {
 fmt.Print("diagnostic start\nok  \tgithub.com/mekugi-harness/mekugi/mekugi-tui/libraries/fixture\tuseful diagnostic\ndiagnostic end\n")
 t.Fatal("actionable failure after noisy passing test")
}
`)
	bin := filepath.Join(t.TempDir(), "bin")
	for name, script := range map[string]string{
		"workspace":     "exec '" + strings.ReplaceAll(executable, "'", "'\\''") + "' -test.run=^TestMakefileWorkspaceFixture$ -- \"$@\"\n",
		"rtk":           "echo 'unexpected RTK filtering' >&2; exit 99\n",
		"golangci-lint": "case \" $* \" in *' --allow-serial-runners '*) ;; *) echo 'missing serial locking' >&2; exit 98;; esac\nprintf 'fixture.go:3: S1003: use strings.Contains (staticcheck)\\n'; exit 7\n",
		"deadcode":      "exit 0\n",
	} {
		path := filepath.Join(bin, name)
		writeWorkspaceSource(t, path, "#!/bin/sh\n"+script)
		if err := os.Chmod(path, 0700); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("MEKUGI_MAKEFILE_WORKSPACE_FIXTURE", "1")
	for _, rtk := range []string{"rtk", ""} {
		for _, target := range []string{"test", "lint", "test-ui-snapshots", "update-ui-snapshots"} {
			t.Run(target+"/RTK="+rtk, func(t *testing.T) {
				command := exec.Command("make", "--no-print-directory", target, "RTK="+rtk, "WORKSPACE="+filepath.Join(bin, "workspace"), "GOWORK="+os.Getenv("GOWORK"),
					"TEST_PACKAGES=./mekugi-tui/libraries/fixture/...", "TEST_SUPPORT_PACKAGES=", "TEST_FLAGS=-count=1", "TEST_RUN=PassingNoise|Failure",
					"SNAPSHOT_PACKAGES=./mekugi-tui/libraries/fixture/...", "SNAPSHOT=PassingNoise|Failure")
				output, err := command.CombinedOutput()
				if err == nil {
					t.Fatalf("recipe succeeded:\n%s", output)
				}
				want := "fixture.go:3: S1003"
				if target != "lint" {
					want = "actionable failure after noisy passing test"
					if !bytes.Contains(output, []byte("FAIL\t"+prefix+"/mekugi-tui/libraries/fixture")) || !bytes.Contains(output, []byte("diagnostic start\nok  \t"+prefix+"/mekugi-tui/libraries/fixture\tuseful diagnostic\ndiagnostic end\n")) {
						t.Fatalf("missing package outcome:\n%s", output)
					}
				}
				if !bytes.Contains(output, []byte(want)) || bytes.Contains(output, []byte("unexpected RTK")) || len(output) > 10000 {
					t.Fatalf("incomplete or unbounded diagnostics (%d bytes):\n%.10000s", len(output), output)
				}
			})
		}
	}
	for _, rtk := range []string{"rtk", ""} {
		command := exec.Command("make", "--no-print-directory", "test", "RTK="+rtk, "WORKSPACE="+filepath.Join(bin, "workspace"), "GOWORK="+os.Getenv("GOWORK"),
			"TEST_PACKAGES=./mekugi-tui/libraries/fixture/...", "TEST_SUPPORT_PACKAGES=", "TEST_FLAGS=-count=1", "TEST_RUN=Passing")
		output, err := command.CombinedOutput()
		compact := bytes.Contains(output, []byte("Go packages: 1 passed, 1 without tests")) && !bytes.Contains(output, []byte("ok  \t")) && !bytes.Contains(output, []byte("?   \t"))
		native := bytes.Contains(output, []byte("ok  \t"+prefix+"/mekugi-tui/libraries/fixture")) && bytes.Contains(output, []byte("[no test files]"))
		if err != nil || (rtk != "" && !compact) || (rtk == "" && !native) || len(output) > 300 {
			t.Fatalf("passing noisy package lost its outcome: RTK=%q error=%v\n%.10000s", rtk, err, output)
		}
	}
}

func TestMakefileFocusedOwnerRecipes(t *testing.T) {
	repositoryMakefile(t)
	for _, tc := range []struct {
		target, packages string
	}{
		{"test-saved-diff", "./mekugi-tui/modules/changehistory/presentation ./mekugi-tui/modules/changehistory/terminalview ./mekugi-tui/libraries/diffrender"},
		{"test-live-preview", "./mekugi-tui/modules/livepreview/..."},
	} {
		t.Run(tc.target, func(t *testing.T) {
			command := exec.Command("make", "--no-print-directory", "-n", tc.target, "RTK=", "TEST_RUN=^FocusedOwner$", "TEST_FLAGS=-count=1")
			command.Dir = "../../../../.."
			output, err := command.CombinedOutput()
			if err != nil {
				t.Fatalf("dry run: %v\n%s", err, output)
			}
			for _, want := range []string{tc.packages, "-run '^FocusedOwner$'", "-count=1"} {
				if !bytes.Contains(output, []byte(want)) {
					t.Fatalf("recipe lost %q:\n%s", want, output)
				}
			}
			if bytes.Contains(output, []byte("./mekugi-tui/apps/mekugi/internal/launch")) {
				t.Fatalf("focused owner recipe includes Launch:\n%s", output)
			}
		})
	}
}

func TestMakefileTestFlagsArguments(t *testing.T) {
	repositoryMakefile(t)
	workspace := filepath.Join(t.TempDir(), "workspace")
	if err := os.WriteFile(workspace, []byte("#!/bin/sh\n[ \"$1\" = check ] && exit 0\nprintf '<%s>\\n' \"$@\"\n"), 0700); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ target, packages string }{
		{"test", "./mekugi-tui/libraries/fixture"},
		{"test-saved-diff", "./mekugi-tui/modules/changehistory/presentation ./mekugi-tui/modules/changehistory/terminalview ./mekugi-tui/libraries/diffrender"},
		{"test-live-preview", "./mekugi-tui/modules/livepreview/..."},
	} {
		t.Run(tc.target, func(t *testing.T) {
			command := exec.Command("make", "--no-print-directory", tc.target, "RTK=", "WORKSPACE="+workspace,
				"TEST_PACKAGES=./mekugi-tui/libraries/fixture", "TEST_SUPPORT_PACKAGES=", "TEST_RUN=^FocusedOwner$",
				"TEST_FLAGS=-bench ^BenchmarkName$ -benchtime=1x -count=1 '-quoted value'")
			command.Dir = "../../../../.."
			output, err := command.CombinedOutput()
			if err != nil {
				t.Fatalf("recipe: %v\n%s", err, output)
			}
			for _, want := range append(strings.Fields(tc.packages), "-run", "^FocusedOwner$", "-bench", "^BenchmarkName$", "-benchtime=1x", "-count=1", "-quoted value") {
				if !bytes.Contains(output, []byte("<"+want+">\n")) {
					t.Fatalf("recipe lost argument %q:\n%s", want, output)
				}
			}
		})
	}
}

func TestMakefileWorkspaceFixture(t *testing.T) {
	if os.Getenv("MEKUGI_MAKEFILE_WORKSPACE_FIXTURE") != "1" {
		return
	}
	for i, arg := range os.Args {
		if arg == "--" {
			os.Args = append([]string{"workspace"}, os.Args[i+1:]...)
			main()
			os.Exit(0)
		}
	}
	t.Fatal("missing workspace arguments")
}
