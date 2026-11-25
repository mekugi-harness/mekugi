package launch

import (
	"bytes"
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/vt"
	"github.com/creack/pty"
)

func TestWrapIntegratedUIAndRedirectedBehavior(t *testing.T) {
	for _, test := range []struct {
		name       string
		terminal   bool
		optOut     bool
		failure    bool
		hookChoice string
	}{
		{name: "terminal", terminal: true},
		{name: "terminal-hook-decline", terminal: true, hookChoice: "1"},
		{name: "terminal-hook-trust", terminal: true, hookChoice: "2"},
		{name: "terminal-env-zero", terminal: true, optOut: true},
		{name: "redirected"},
		{name: "terminal-failure", terminal: true, failure: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			dir := t.TempDir()
			cache := shortTempDir(t)
			t.Setenv("HOME", cache) // os.UserCacheDir uses HOME on macOS.
			t.Setenv("XDG_CACHE_HOME", cache)
			launcher := os.Args[0]
			workspace := t.TempDir()
			rpcMarker := filepath.Join(dir, "app-server-rpc")
			t.Setenv("MEKUGI_AUTO_WRAP_PROCESS", "1")
			t.Setenv("MEKUGI_AUTO_WRAP_HOOK_CHOICE", test.hookChoice)
			t.Setenv("MEKUGI_AUTO_WRAP_TERMINAL", "0")
			t.Setenv("MEKUGI_AUTO_WRAP_FAILURE", "0")
			if test.failure {
				t.Setenv("MEKUGI_AUTO_WRAP_FAILURE", "1")
			}
			if test.terminal {
				t.Setenv("MEKUGI_AUTO_WRAP_TERMINAL", "1")
			}
			t.Setenv("MEKUGI_AUTO_WRAP_RPC_MARKER", rpcMarker)
			t.Setenv("MEKUGI_APP_SERVER_UI", "")
			if test.optOut {
				t.Setenv("MEKUGI_APP_SERVER_UI", "0")
			} else if err := os.Unsetenv("MEKUGI_APP_SERVER_UI"); err != nil {
				t.Fatal(err)
			}
			t.Setenv("MEKUGI_AUTO_WRAP_WORKSPACE", workspace)
			t.Setenv("CODEX_HOME", t.TempDir())
			t.Setenv("XDG_CONFIG_HOME", t.TempDir())
			t.Setenv("XDG_STATE_HOME", t.TempDir())
			t.Setenv("MEKUGI_RUNTIME_DIR", t.TempDir())
			// Keep installed runtime tools such as Node.js available.
			t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
			for name, script := range map[string]string{
				"mekugi-app-server": "#!/bin/sh\nexec '" + strings.ReplaceAll(os.Args[0], "'", "'\\''") + "' -test.run=^TestAutoWrapProcess$ -- codex \"$@\"\n",
			} {
				if err := os.WriteFile(filepath.Join(dir, name), []byte(script), 0700); err != nil {
					t.Fatal(err)
				}
			}
			ctx, cancel := context.WithTimeout(t.Context(), 30*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, launcher, "-test.run=^TestAutoWrapProcess$", "--", "router")
			var output []byte
			var err error
			var sent []string
			if test.terminal {
				tty, startErr := pty.StartWithSize(cmd, &pty.Winsize{Rows: 24, Cols: 100})
				if startErr != nil {
					t.Fatal(startErr)
				}
				defer tty.Close()
				read := make(chan []byte, 1)
				go func() {
					var data bytes.Buffer
					screen := vt.NewEmulator(100, 24)
					stopReplies := startEmulatorReplies(io.Discard, screen)
					buf := make([]byte, 8192)
					quit, answeredHook := false, false
					for {
						n, readErr := tty.Read(buf)
						data.Write(buf[:n])
						_, _ = screen.Write(buf[:n])
						visible := screen.String()
						promptOpen := strings.Contains(visible, "Trust this hook?")
						if test.hookChoice != "" && !answeredHook && promptOpen {
							sent = append(sent, test.hookChoice+"\r")
							_, _ = tty.Write([]byte(test.hookChoice + "\r"))
							answeredHook = true
						} else if !test.failure && !quit && (test.hookChoice == "" || answeredHook) && !promptOpen && composerStatusShows(visible, "Ready") {
							sent = append(sent, "/quit\r")
							_, _ = tty.Write([]byte("/quit\r"))
							quit = true
						}
						if readErr != nil {
							break
						}
					}
					stopReplies()
					read <- data.Bytes()
				}()
				err = cmd.Wait()
				output = <-read
			} else {
				output, err = cmd.CombinedOutput()
			}
			if test.failure {
				if err == nil || cmd.ProcessState.ExitCode() != 23 || !bytes.Contains(output, []byte("app-server disconnected; active work may be incomplete")) {
					t.Fatalf("unexpected app-server exit lost its status or diagnostic: %v\n%s", err, output)
				}
			} else if !test.terminal {
				if err == nil || cmd.ProcessState.ExitCode() != 2 || !bytes.Contains(output, []byte("interactive sessions require terminal stdin and stdout")) {
					t.Fatalf("redirected ordinary invocation was not rejected: %v\n%s", err, output)
				}
			} else if err != nil {
				if test.hookChoice != "" {
					screen := vt.NewEmulator(100, 24)
					stopReplies := startEmulatorReplies(io.Discard, screen)
					defer stopReplies()
					_, _ = screen.Write(output)
					t.Fatalf("wrapper: %v; sent inputs: %q\n%s", err, sent, screen.String())
				}
				t.Fatalf("wrapper: %v\n%s", err, output)
			}
			if test.terminal {
				// The second stat line leads with the painted "Main" label.
				if !bytes.Contains(output, []byte("\x1b[?1049h")) || !bytes.Contains(output, []byte("mMain\x1b")) {
					t.Fatalf("terminal wrapper did not render native Main: %q", output)
				}
				calls, readErr := os.ReadFile(rpcMarker)
				handshake := "initialize\ninitialized\nenvironment/info\nthread/start\n"
				if readErr != nil || !bytes.Contains(calls, []byte(handshake)) {
					t.Fatalf("native app-server handshake: %q, %v", calls, readErr)
				}
				if !test.failure && !bytes.Contains(calls, []byte("hooks/list\n")) {
					t.Fatalf("hook inventory not requested: %s", calls)
				}
				if test.hookChoice != "" && bytes.Contains(calls, []byte("config/batchWrite\n")) != (test.hookChoice == "2") {
					t.Fatalf("hook trust decision: %s", calls)
				}
				if test.hookChoice != "" && (!bytes.Contains(output, []byte("Trust: modified")) || !bytes.Contains(output, []byte("bash scripts/startup.sh"))) {
					t.Fatal("rendered startup hook review omitted changed trust status or command")
				}
			} else if bytes.Contains(output, []byte("\x1b[?1049h")) || bytes.Contains(output, []byte("Main • ")) {
				t.Fatalf("redirected wrapper unexpectedly rendered the integrated UI: %q", output)
			} else if _, statErr := os.Stat(rpcMarker); !os.IsNotExist(statErr) {
				t.Fatalf("redirected wrapper unexpectedly started app-server: %v", statErr)
			}
			if !test.terminal {
				if bytes.ContainsAny(output, "\x1b\x07") {
					t.Fatalf("launcher emitted provider terminal controls: %q", output)
				}
			}
		})
	}
}

func TestInteractiveCodexArgs(t *testing.T) {
	for _, test := range []struct {
		args []string
		want bool
	}{
		{nil, true}, {[]string{"resume", "--last"}, true}, {[]string{"--model", "exec", "hello"}, true},
		{[]string{"exec", "hello"}, false}, {[]string{"--model", "gpt-6", "exec", "hello"}, false},
		{[]string{"--help"}, false}, {[]string{"--version"}, false}, {[]string{"-c", "help", "prompt"}, true},
	} {
		if got := interactiveCodexArgs(test.args); got != test.want {
			t.Errorf("%v: %v, want %v", test.args, got, test.want)
		}
	}
}
