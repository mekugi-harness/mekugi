package launch

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
)

// TestDirectNativeRealAppServerIntegration exercises the real Rust
// mekugi-app-server binary over ws+unix through directNativeConnection without
// requiring model credentials or external network access.
func TestDirectNativeRealAppServerIntegration(t *testing.T) {
	bin, err := exec.LookPath("mekugi-app-server")
	if err != nil {
		if os.Getenv("MEKUGI_REQUIRE_NATIVE_INTEGRATION") != "" {
			t.Fatalf("MEKUGI_REQUIRE_NATIVE_INTEGRATION is set, but mekugi-app-server not found on PATH: %v", err)
		}
		t.Skip("mekugi-app-server not on PATH")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	workspace := t.TempDir()
	if err := os.WriteFile(filepath.Join(workspace, "hello_integration.go"), []byte("package demo\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	skillDir := filepath.Join(workspace, ".agents", "skills", "demo-skill")
	if err := os.MkdirAll(skillDir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte("---\nname: demo-skill\ndescription: Integration test skill\n---\n\nDemo.\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	codexHome := t.TempDir()
	xdgConfig := t.TempDir()
	xdgState := t.TempDir()
	socketPath := filepath.Join(shortTempDir(t), "native.sock")
	proc := exec.Command(bin, "--listen", "unix://"+socketPath, "-c", "features.plugins=false")
	proc.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	proc.Dir = workspace
	proc.Env = append(os.Environ(),
		"CODEX_HOME="+codexHome,
		"XDG_CONFIG_HOME="+xdgConfig,
		"XDG_STATE_HOME="+xdgState,
	)
	var stderr bytes.Buffer
	proc.Stderr = &stderr
	if err := proc.Start(); err != nil {
		t.Fatalf("start mekugi-app-server: %v", err)
	}
	defer func() {
		if proc.Process != nil {
			pgid := proc.Process.Pid
			_ = syscall.Kill(-pgid, syscall.SIGTERM)
			done := make(chan struct{})
			go func() {
				_ = proc.Wait()
				close(done)
			}()
			select {
			case <-done:
			case <-time.After(2 * time.Second):
				_ = syscall.Kill(-pgid, syscall.SIGKILL)
				<-done
			}
			waitDeadline := time.Now().Add(2 * time.Second)
			for time.Now().Before(waitDeadline) {
				if err := syscall.Kill(-pgid, 0); err != nil {
					break
				}
				time.Sleep(10 * time.Millisecond)
			}
		}
	}()

	deadline := time.Now().Add(15 * time.Second)
	for {
		if _, err := os.Stat(socketPath); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("mekugi-app-server socket %s not ready: %s", socketPath, stderr.String())
		}
		time.Sleep(25 * time.Millisecond)
	}

	conn, err := openDirectNativeConnection(ctx, socketPath, workspace, nil, nil, "", "")
	if err != nil {
		t.Fatalf("openDirectNativeConnection: %v\nstderr:\n%s", err, stderr.String())
	}
	defer conn.Close()

	drain := func() sessionapi.NativeEvent {
		var last sessionapi.NativeEvent
		for {
			select {
			case ev := <-conn.events:
				last = ev
			default:
				return last
			}
		}
	}
	awaitEvent := func(desc string, match func(sessionapi.NativeEvent) bool) sessionapi.NativeEvent {
		t.Helper()
		timer := time.NewTimer(5 * time.Second)
		defer timer.Stop()
		for {
			select {
			case ev := <-conn.events:
				if match(ev) {
					return ev
				}
			case <-timer.C:
				t.Fatalf("timed out waiting for %s", desc)
			}
		}
	}

	initial := drain()
	if initial.Frame.View.Main.ID == "" {
		initial = awaitEvent("initial main thread", func(ev sessionapi.NativeEvent) bool {
			return ev.Frame.View.Main.ID != ""
		})
	}
	mainID := initial.Frame.View.Main.ID
	if initial.Frame.View.Main.Caller != "/root" {
		t.Fatalf("expected root caller /root, got %q", initial.Frame.View.Main.Caller)
	}

	// 1. model/list via /model opens MenuSettings with real models.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/model")}); err != nil {
		t.Fatalf("/model: %v", err)
	}
	ev := drain()
	if ev.Frame.View.Menu == nil || len(ev.Frame.View.Menu.Choices) == 0 {
		t.Fatalf("expected non-empty /model menu from real mekugi-app-server, got %+v", ev.Frame.View.Menu)
	}

	// 2. skills/list via $ completion discovers the workspace skill.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Completion: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 1},
			Draft:  composer.Draft{Text: "$demo"},
		},
	}); err != nil {
		t.Fatalf("$ completion: %v", err)
	}
	ev = drain()
	foundSkill := false
	if ev.Frame.View.Menu != nil {
		for _, ch := range ev.Frame.View.Menu.Choices {
			if ch.Name == "$demo-skill" {
				foundSkill = true
				break
			}
		}
	}
	if !foundSkill {
		t.Fatalf("expected $demo-skill from real skills/list, got %+v", ev.Frame.View.Menu)
	}

	// 3. fuzzyFileSearch via @ completion discovers hello_integration.go.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Completion: &sessionapi.NativeEditorInput{
			Target: conversation.EditorTarget{Revision: 2},
			Draft:  composer.Draft{Text: "@hello_integration"},
		},
	}); err != nil {
		t.Fatalf("@ completion: %v", err)
	}
	ev = drain()
	foundFile := false
	if ev.Frame.View.Menu != nil {
		for _, ch := range ev.Frame.View.Menu.Choices {
			if strings.Contains(ch.Name, "hello_integration.go") {
				foundFile = true
				break
			}
		}
	}
	if !foundFile {
		t.Fatalf("expected hello_integration.go from real fuzzyFileSearch, got %+v", ev.Frame.View.Menu)
	}

	// 4. thread/settings/update (/effort high) updates real server settings and delivers thread/settings/updated.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/effort high")}); err != nil {
		t.Fatalf("/effort high: %v", err)
	}
	awaitEvent("effort=high from thread/settings/updated", func(ev sessionapi.NativeEvent) bool {
		return ev.Frame.View.Caption.Effort == "high"
	})

	// 5. thread/name/set (/title) updates thread name.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/title Cross-Boundary OK")}); err != nil {
		t.Fatalf("/title: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Info.Title != "Cross-Boundary OK" {
		t.Fatalf("expected title Cross-Boundary OK, got %q", ev.Frame.View.Info.Title)
	}

	// 6. /ps calls real thread/backgroundTerminals/list cleanly.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/ps")}); err != nil {
		t.Fatalf("/ps: %v", err)
	}
	ev = drain()
	if ev.Frame.Details == nil || !ev.Frame.Details.View.Processes {
		t.Fatalf("expected /ps to open Processes detail dialog against real app-server")
	}

	// 7. /fork calls real thread/fork and switches mainThread to a new ID.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/fork")}); err != nil {
		t.Fatalf("/fork: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Main.ID == "" || ev.Frame.View.Main.ID == mainID {
		t.Fatalf("expected /fork to switch to new thread ID, still %q", ev.Frame.View.Main.ID)
	}

	// 8. /archive calls real thread/archive cleanly.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/archive")}); err != nil {
		t.Fatalf("/archive: %v", err)
	}
	ev = drain()
	if !ev.Frame.Facts.Archived {
		t.Fatalf("expected /archive to mark thread archived")
	}
}
