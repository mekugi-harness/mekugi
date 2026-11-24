package main

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// installJustFixture reads the repository recipes through a testdata symlink
// so Go's test cache tracks edits, then routes workspace calls to this binary.
func installJustFixture(t *testing.T) string {
	t.Helper()
	recipes, err := os.ReadFile("testdata/justfile")
	if err != nil {
		t.Fatal(err)
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	bin := t.TempDir()
	t.Setenv("MEKUGI_JUST_FIXTURE", executable)
	t.Setenv("MEKUGI_JUST_REAL_GO", realGo)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	path := filepath.Join(bin, "go")
	writeWorkspaceSource(t, path, `#!/bin/sh
if [ "${1-} ${2-} ${3-}" = "run -trimpath ./mekugi-tui/apps/mekugi/cmd/workspace" ]; then
    shift 3
    exec "$MEKUGI_JUST_FIXTURE" -test.run=^TestJustWorkspaceFixture$ -- "$@"
fi
exec "$MEKUGI_JUST_REAL_GO" "$@"
`)
	if err := os.Chmod(path, 0700); err != nil {
		t.Fatal(err)
	}
	return string(recipes)
}

func TestJustValidationOutput(t *testing.T) {
	recipes := installJustFixture(t)
	workspaceFixture(t, []string{"./mekugi-tui/libraries/fixture", "./mekugi-tui/testsupport/uisnapshot"}, map[string]string{
		"mekugi-tui/libraries/fixture":      prefix + "/mekugi-tui/libraries/fixture",
		"mekugi-tui/testsupport/uisnapshot": prefix + "/mekugi-tui/testsupport/uisnapshot",
	})
	writeWorkspaceSource(t, "justfile", recipes)
	writeWorkspaceSource(t, "mekugi-tui/go.work", "go 1.27.0\nuse (\n./libraries/fixture\n./testsupport/uisnapshot\n)\n")
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
	bin := t.TempDir()
	for name, script := range map[string]string{
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
	for _, rtk := range []string{"rtk", ""} {
		for _, target := range []string{"test", "lint", "test-ui-snapshots", "update-ui-snapshots"} {
			t.Run(target+"/RTK="+rtk, func(t *testing.T) {
				args := []string{target}
				switch target {
				case "test":
					args = append(args, "./mekugi-tui/libraries/fixture/...", "-count=1", "-run", "PassingNoise|Failure")
				case "test-ui-snapshots", "update-ui-snapshots":
					args = append(args, "./mekugi-tui/libraries/fixture/...", "PassingNoise|Failure")
				}
				t.Setenv("RTK", rtk)
				command := exec.Command("just", args...)
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
		t.Setenv("RTK", rtk)
		command := exec.Command("just", "test", "./mekugi-tui/libraries/fixture/...", "-count=1", "-run", "Passing")
		output, err := command.CombinedOutput()
		compact := bytes.Contains(output, []byte("Go packages: 1 passed, 1 without tests")) && !bytes.Contains(output, []byte("ok  \t")) && !bytes.Contains(output, []byte("?   \t"))
		native := bytes.Contains(output, []byte("ok  \t"+prefix+"/mekugi-tui/libraries/fixture")) && bytes.Contains(output, []byte("[no test files]"))
		if err != nil || (rtk != "" && !compact) || (rtk == "" && !native) || len(output) > 300 {
			t.Fatalf("passing noisy package lost its outcome: RTK=%q error=%v\n%.10000s", rtk, err, output)
		}
	}
}

func TestJustTestArguments(t *testing.T) {
	recipes := installJustFixture(t)
	t.Chdir(t.TempDir())
	writeWorkspaceSource(t, "justfile", recipes)
	t.Setenv("RTK", "")
	t.Setenv("MEKUGI_JUST_PRINT_ARGS", "1")
	args := []string{"./mekugi-tui/libraries/fixture", "-run", "^FocusedOwner$",
		"-ldflags=-X example.test.version=test version", "-count=1"}
	command := exec.Command("just", append([]string{"test"}, args...)...)
	output, err := command.CombinedOutput()
	if err != nil {
		t.Fatalf("recipe: %v\n%s", err, output)
	}
	var want bytes.Buffer
	for _, arg := range append([]string{"run", "go", "test", "-trimpath", "-parallel=32"}, args...) {
		fmt.Fprintf(&want, "<%s>\n", arg)
	}
	if !bytes.Equal(output, want.Bytes()) {
		t.Fatalf("unexpected argv:\n%s\nwant:\n%s", output, &want)
	}
}

func TestJustWorkspaceFixture(t *testing.T) {
	if os.Getenv("MEKUGI_JUST_FIXTURE") == "" {
		return
	}
	for i, arg := range os.Args {
		if arg == "--" {
			args := os.Args[i+1:]
			if os.Getenv("MEKUGI_JUST_PRINT_ARGS") == "1" {
				if len(args) != 1 || args[0] != "check" {
					for _, arg := range args {
						fmt.Printf("<%s>\n", arg)
					}
				}
			} else {
				os.Args = append([]string{"workspace"}, args...)
				main()
			}
			os.Exit(0)
		}
	}
	t.Fatal("missing workspace arguments")
}
