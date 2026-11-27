//go:build codex_e2e

package launch

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/charmbracelet/x/vt"
)

func TestNativeApprovalDenialSameTurnCodexE2E(t *testing.T) {
	const reason = "Use the existing result instead."
	shell := execTrackShellExecutable(t, "bash")
	workspace := t.TempDir()
	const marker = "approval-command-ran"
	writeHostFixture(t, filepath.Join(workspace, "git"), "#!/bin/sh\ntouch "+marker+"\n", 0700)
	const denied = "remote write denied: " + reason
	var original fixtureRequestMetadata
	requests := 0
	provider := &toolFrontendCodexProvider{
		program:   hostTrackedCommand(shell, "./git push origin main", 1),
		expected:  []string{denied, "HOST_RESULT_VERIFIED"},
		finalText: "Recovered after a retry.",
		observeRequest: func(body []byte, headers http.Header) error {
			requests++
			metadata, ok := fixtureTurnMetadata(headers)
			if !ok || metadata.ThreadID == "" || metadata.TurnID == "" {
				return fmt.Errorf("missing native thread/turn identity")
			}
			if requests == 1 {
				original = metadata
				return nil
			}
			if requests != 2 || metadata.ThreadID != original.ThreadID || metadata.TurnID != original.TurnID {
				return fmt.Errorf("denial continuation changed turn or added request: requests=%d original=%s/%s current=%s/%s", requests, original.ThreadID, original.TurnID, metadata.ThreadID, metadata.TurnID)
			}
			var request struct {
				Input []struct {
					Type   string         `json:"type"`
					CallID string         `json:"call_id"`
					Output jsontext.Value `json:"output"`
				} `json:"input"`
			}
			if err := json.Unmarshal(body, &request); err != nil {
				return err
			}
			denials := 0
			for _, item := range request.Input {
				if item.Type == "custom_tool_call_output" && item.CallID == "frontend-call" {
					denials += strings.Count(string(item.Output), denied)
				}
			}
			if denials != 1 {
				return fmt.Errorf("first same-turn follow-up contains %d guarded denials; want one with the supplied reason", denials)
			}
			return nil
		},
	}
	runAppServerPreviewWith(t, provider, nil, appServerPreview{
		managedRuntime: true,
		workspace:      workspace,
		// The shell guard needs its Unix socket, independently of the sandbox.
		codexArgs:     []string{"-c", `sandbox_mode="danger-full-access"`},
		completedText: provider.finalText,
		duringTurn: func(t *testing.T, outer io.Writer, await func(string), _ func(func(string) bool), _ *vt.Emulator) {
			await("Approve guarded shell command")
			if _, err := io.WriteString(outer, reason+"\r"); err != nil {
				t.Fatal(err)
			}
		},
	})
	if _, err := os.Stat(filepath.Join(workspace, marker)); !os.IsNotExist(err) {
		t.Fatalf("denied command ran: %v", err)
	}
	provider.mu.Lock()
	defer provider.mu.Unlock()
	if provider.turns != 2 || !provider.resultSeen {
		t.Fatalf("native denial acceptance: requests=%d result=%t output=%s", provider.turns, provider.resultSeen, provider.output)
	}
}
