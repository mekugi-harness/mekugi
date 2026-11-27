//go:build codex_e2e

package launch

import (
	"bytes"
	"context"
	"io"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/vt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/connector"
	"github.com/mekugi-harness/mekugi/mekugi-webui"
)

func TestWaitingEditsAndPermissionsThroughNativeBrowser(t *testing.T) {
	if os.Getenv("MEKUGI_TEST_WEBUI_BROWSER") != "1" {
		t.Skip("set MEKUGI_TEST_WEBUI_BROWSER=1 after just webui")
	}
	t.Setenv("AGENT_BROWSER_ENGINE", "chrome")
	browser := browserFixtureCommand(t)
	ctx, cancel := context.WithTimeout(t.Context(), 90*time.Second)
	defer cancel()
	id, err := exec.CommandContext(ctx, browser, "session", "id", "--scope", "worktree", "--prefix", "waiting-edits").Output()
	if err != nil {
		t.Fatal(err)
	}
	session := strings.TrimSpace(string(id))
	defer func() {
		cleanup, stop := context.WithTimeout(context.Background(), 10*time.Second)
		defer stop()
		_ = exec.CommandContext(cleanup, browser, "--session", session, "close").Run()
	}()
	call := func(args ...string) {
		t.Helper()
		if out, err := exec.CommandContext(ctx, browser, append([]string{"--session", session}, args...)...).CombinedOutput(); err != nil {
			snap, _ := exec.CommandContext(ctx, browser, "--session", session, "snapshot").CombinedOutput()
			t.Fatalf("browser %v: %v: %s\n%s", args, err, out, snap)
		}
	}
	page := httptest.NewServer(webui.Handler())
	defer page.Close()
	const token = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
	const original = "original waiting browser message"
	const edited = "edited waiting browser message"
	tokenFile := filepath.Join(shortTempDir(t), "token")
	if err := os.WriteFile(tokenFile, []byte(token+"\n"), 0600); err != nil {
		t.Fatal(err)
	}
	socket := make(chan string, 1)
	integration := appServerIntegration{controlSocket: socket}
	gate := make(chan struct{})
	provider := &appQueueProvider{requests: make(chan []byte, 8), gates: []chan struct{}{gate}}
	next := func() []byte {
		t.Helper()
		select {
		case body := <-provider.requests:
			return body
		case <-ctx.Done():
			t.Fatal(ctx.Err())
			return nil
		}
	}
	call("open", page.URL)
	call("set", "viewport", "1280", "900")
	call("find", "label", "Machine name", "fill", "Waiting fixture")
	call("find", "label", "Access token", "fill", token)
	runAppServerPreviewWith(t, provider, nil, appServerPreview{
		managedRuntime: true, integration: &integration, completedText: "Recovered after a retry.", saveBeforePrompt: true,
		environment: []string{"SHELL=/bin/bash", "CODEX_HOME=" + t.TempDir()},
		beforePrompt: func(t *testing.T, _ io.Writer, _ func(string), _ func(func(string) bool), _ *vt.Emulator) {
			bridge, err := connector.New(connector.Options{SocketPath: <-socket, TokenFile: tokenFile, Origins: []string{page.URL}})
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = bridge.Close() })
			remote := httptest.NewServer(bridge)
			t.Cleanup(remote.Close)
			call("find", "label", "Connector URL", "fill", remote.URL)
			call("find", "role", "button", "click", "--name", "Connect", "--exact")
			call("wait", "nav[aria-label=Threads] li a")
			call("click", "nav[aria-label=Threads] li a")
			call("wait", "#composer-text")
			call("fill", "#composer-text", "draft survives settings")
			call("wait", `button[title="Permissions"]`)
			call("find", "role", "combobox", "click", "--name", "Permissions", "--exact")
			call("find", "role", "option", "click", "--name", ":read-only", "--exact")
			call("wait", "--fn", `document.querySelector('button[title="Permissions"]')?.textContent.includes(':read-only') && document.querySelector('#composer-text')?.value === 'draft survives settings'`)
			call("wait", `[role="listbox"][aria-label="Permissions"]`, "--state", "hidden")
		},
		duringTurn: func(t *testing.T, _ io.Writer, _ func(string), _ func(func(string) bool), _ *vt.Emulator) {
			defer func() {
				select {
				case <-gate:
				default:
					close(gate)
				}
			}()
			if !bytes.Contains(next(), []byte("Run preview fixture")) {
				t.Fatal("the native turn did not reach the provider")
			}
			call("wait", `button[aria-label="Steer turn"]`)
			call("set", "viewport", "1120", "900")
			call("find", "role", "button", "click", "--name", "Changes", "--exact")
			call("wait", "--fn", `(() => {
				const form = document.querySelector('#composer-text')?.closest('form');
				const bounds = form?.getBoundingClientRect();
				const controls = [...(form?.querySelectorAll('button') ?? [])].filter(
					button => /^(Attach|Permissions|Interrupt turn|Steer turn)$/.test(button.ariaLabel ?? button.title));
				return !!document.querySelector('nav[aria-label=Threads]') &&
					[...document.querySelectorAll('aside')].some(panel => panel.textContent.includes('Saved Diff')) &&
					controls.length >= 3 && controls.every(button => {
						const box = button.getBoundingClientRect();
						return box.width > 0 && box.left >= bounds.left && box.right <= bounds.right;
					});
			})()`)
			call("find", "role", "button", "click", "--name", "Changes", "--exact")
			call("set", "viewport", "1280", "900")
			call("find", "role", "button", "click", "--name", "Session details", "--exact")
			call("wait", "--text", "This launch")
			call("focus", `[aria-label="Session details"] summary`)
			call("press", "Enter")
			call("wait", "--fn", `[...document.querySelectorAll('[aria-label="Session details"] th')].some(th => th.textContent === 'Descendants') && [...document.querySelectorAll('[aria-label="Session details"] th')].some(th => th.textContent === 'Output deduplication')`)
			call("find", "role", "button", "click", "--name", "Archive", "--exact")
			call("wait", "--text", "This will stop the current turn and archive the thread.")
			call("find", "role", "button", "click", "--name", "Cancel", "--exact")
			call("fill", "#composer-text", original)
			call("press", "Tab")
			call("wait", "--text", original)
			call("find", "role", "button", "click", "--name", "Edit queued messages", "--exact")
			call("wait", `[aria-label="Draft recovery"] textarea`)
			call("fill", `[aria-label="Draft recovery"] textarea`, edited)
			call("find", "role", "button", "click", "--name", "Save to queue", "--exact")
			call("wait", "--text", edited)
			call("find", "role", "button", "click", "--name", "Steer queued messages", "--exact")
			call("wait", "--fn", `!document.querySelector('[aria-label="Queued messages"]')`)
			call("fill", "#composer-text", "/compact")
			call("press", "Tab")
			call("wait", "--fn", `document.querySelector('#composer-text').value.trim()==='/compact'`)
			call("press", "Tab")
			call("wait", "--fn", `document.querySelector('[aria-label="Queued messages"]')?.textContent.includes('/compact')`)
			if len(provider.requests) != 0 {
				t.Fatal("Tab submitted compaction before the running turn reached idle")
			}
			close(gate)
			for {
				body := next()
				if bytes.Contains(body, []byte(original)) {
					t.Fatal("the pre-edit message reached the model")
				}
				if bytes.Contains(body, []byte(edited)) {
					break
				}
			}
			call("wait", "--text", "Context reset")
			next() // The successful compact continues even when no fresh draft is waiting.
			call("wait", "--fn", `!document.querySelector('[aria-label="Queued messages"]') &&
				![...document.querySelectorAll('[data-entry]')].some(row=>row.textContent.trim().endsWith('/compact'))`)
		},
		afterPrompt: func(t *testing.T, _ io.Writer, _ func(string), _ func(func(string) bool), _ *vt.Emulator) {
			const scroller = `document.querySelector('[aria-label="Side answer"] .overflow-y-auto')`
			paragraphs := strings.Repeat("\n\nSide question context for the separate answer.", 16)
			for _, question := range []string{"What is a separate answer?", "A distinct follow-up?"} {
				call("fill", "#composer-text", "/btw "+question+paragraphs)
				call("press", "Enter")
				call("wait", "--fn", `(() => { const dock = document.querySelector('[aria-label="Side answer"]'); return dock?.textContent.includes('`+question+`') && dock.textContent.includes('Recovered after a retry.') && !dock.textContent.includes('Answering…') && document.querySelector('#composer-text')?.value === ''; })()`)
				for !bytes.Contains(next(), []byte(question)) {
				}
				if question == "What is a separate answer?" {
					call("wait", "--fn", `(() => { const s = `+scroller+`; return s?.scrollTop > 0 && s.scrollHeight - s.clientHeight - s.scrollTop < 2; })()`)
					call("eval", `(() => { const s = `+scroller+`; s.scrollTop = 0; s.dispatchEvent(new Event('scroll')); })()`)
				} else {
					call("wait", "--fn", scroller+`?.scrollTop === 0`)
				}
			}
			call("wait", "--fn", `document.querySelector('[aria-label="Side answer"]')?.textContent.includes('What is a separate answer?') && document.querySelector('[aria-label="Side answer"]')?.textContent.includes('Recovered after a retry.') && document.querySelectorAll('nav[aria-label="Threads"] li a').length === 1 && ![...document.querySelectorAll('[data-entry]')].some(entry => entry.textContent.includes('What is a separate answer?'))`)
			call("find", "role", "button", "click", "--name", "Close answer", "--exact")
			call("wait", `[aria-label="Side answer"]`, "--state", "hidden")
			const sideTask = "/literal work in One-off"
			call("fill", "#composer-text", "/oneoff "+sideTask)
			call("press", "Enter")
			call("wait", "--fn", `document.querySelector('[role=tab][aria-selected=true]')?.textContent.trim() === 'One-off' && [...document.querySelectorAll('[data-entry]')].some(row => row.textContent.includes('`+sideTask+`'))`)
			for !bytes.Contains(next(), []byte(sideTask)) {
			}
			call("wait", "--fn", `!document.querySelector('button[aria-label="Steer turn"]') && ![...document.querySelectorAll('[data-entry]')].some(row => row.textContent.includes('Run preview fixture'))`)
			call("fill", "#composer-text", "separate side draft")
			call("find", "role", "tab", "click", "--name", "Main", "--exact")
			call("wait", "--fn", `document.querySelector('[role=tab][aria-selected=true]')?.textContent.trim() === 'Main' && document.querySelector('#composer-text')?.value === ''`)
			call("fill", "#composer-text", "separate main draft")
			call("find", "role", "tab", "click", "--name", "One-off", "--exact")
			call("wait", "--fn", `document.querySelector('#composer-text')?.value === 'separate side draft'`)
			call("press", "ArrowLeft")
			call("wait", "--fn", `document.activeElement?.textContent.trim() === 'Main' && document.querySelector('[role=tab][aria-selected=true]')?.textContent.trim() === 'One-off'`)
			const replacement = "fresh replacement task"
			call("fill", "#composer-text", "/oneoff "+replacement)
			call("press", "Enter")
			call("find", "role", "button", "click", "--name", "Cancel", "--exact")
			call("wait", "--fn", `document.querySelector('#composer-text')?.value === '/oneoff `+replacement+`'`)
			call("focus", "#composer-text")
			call("press", "Enter")
			call("find", "role", "button", "click", "--name", "Replace", "--exact")
			call("wait", "--fn", `[...document.querySelectorAll('[data-entry]')].some(row => row.textContent.includes('`+replacement+`')) && ![...document.querySelectorAll('[data-entry]')].some(row => row.textContent.includes('`+sideTask+`'))`)
			for body := next(); ; body = next() {
				if bytes.Contains(body, []byte(replacement)) {
					if bytes.Contains(body, []byte(sideTask)) {
						t.Fatal("replacement inherited the earlier One-off, not Main")
					}
					break
				}
			}
			call("find", "role", "button", "click", "--name", "Close One-off", "--exact")
			call("wait", "--fn", `!document.querySelector('[role=tablist][aria-label=Conversation]') && document.querySelector('#composer-text')?.value === 'separate main draft' && document.querySelectorAll('nav[aria-label="Threads"] li a').length === 1`)
			call("fill", "#composer-text", "/archive")
			call("press", "Enter")
			call("wait", "--text", "Archive thread?")
			call("find", "role", "button", "click", "--name", "Archive", "--exact")
			call("wait", "--fn", `!document.querySelector('nav[aria-label="Threads"] li a')`)
		},
	})
}
