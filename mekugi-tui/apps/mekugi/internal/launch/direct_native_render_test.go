package launch

import (
	"context"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/composition"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func TestDirectNativeProjectsIntoTerminalSessionFrame(t *testing.T) {
	ws := t.TempDir()
	sampleFile := filepath.Join(ws, "sample.go")
	if err := os.WriteFile(sampleFile, []byte("package demo\n\nfunc Hello() string { return \"hi\" }\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	socket := filepath.Join(shortTempDir(t), "native.sock")
	srv := startMockNativeAppServer(t, socket)
	srv.mu.Lock()
	srv.bgTerminals = []map[string]any{
		{
			"itemId":    "cmd-bg",
			"processId": "proc-42",
			"command":   "/bin/bash -lc 'tail -f app.log'",
			"cwd":       ws,
		},
	}
	srv.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	conn, err := openDirectNativeConnection(ctx, socket, ws, nil, nil, "", "")
	if err != nil {
		t.Fatalf("openDirectNativeConnection: %v", err)
	}
	defer conn.Close()
	<-srv.ready

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

	// Verify /effort is listed and /reasoning is not; /rewind is listed.
	hasEffort, hasReasoning, hasRewind := false, false, false
	for _, ch := range directSlashCommands {
		switch ch.Name {
		case "/effort":
			hasEffort = true
		case "/reasoning":
			hasReasoning = true
		case "/rewind":
			hasRewind = true
		}
	}
	if !hasEffort || hasReasoning || !hasRewind {
		t.Fatalf("unexpected slash catalog flags: effort=%v reasoning=%v rewind=%v", hasEffort, hasReasoning, hasRewind)
	}

	// Push real codex-rs wire shapes: commandExecution, fileChange, assistant reply, tokenUsage (main + worker).
	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{
			"threadId":"thr-main","turnId":"turn-1",
			"item":{
				"type":"commandExecution","id":"cmd-1",
				"command":"/bin/bash -lc 'echo hello-world'",
				"status":"completed","exitCode":0,
				"aggregatedOutput":"line-1\nline-2\nline-3\nline-4\nline-5\nline-6\nline-7\nhello-world\n"
			}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{
			"threadId":"thr-main","turnId":"turn-1",
			"item":{
				"type":"fileChange","id":"fc-1","status":"completed",
				"changes":[{"path":"main.go","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-old\n+new\n"}]
			}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{
			"threadId":"thr-main","turnId":"turn-1",
			"item":{"type":"agentMessage","id":"msg-1","text":"Done fixing regressions."}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "thread/tokenUsage/updated",
		Params: jsontext.Value(`{
			"threadId":"thr-main",
			"tokenUsage":{
				"total":{"totalTokens":15000,"inputTokens":12000,"outputTokens":3000,"cachedInputTokens":1000,"reasoningOutputTokens":200},
				"last":{"totalTokens":100000},
				"modelContextWindow":400000,
				"estimatedCostUsd":0.12,
				"costPartial":false,
				"outputTokensPerSecond":96.0,
				"workDurationMs":1500,
				"roundtrips":2
			}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "thread/tokenUsage/updated",
		Params: jsontext.Value(`{
			"threadId":"thr-worker",
			"tokenUsage":{
				"total":{"totalTokens":5000,"inputTokens":4000,"outputTokens":1000,"cachedInputTokens":0,"reasoningOutputTokens":0},
				"last":{"totalTokens":10000},
				"modelContextWindow":200000,
				"estimatedCostUsd":0.36,
				"costPartial":false,
				"outputTokensPerSecond":64.0,
				"workDurationMs":800,
				"roundtrips":1
			}
		}`),
	})
	ev := drain()

	// Feed the projected NativeFrame through the real nativeClientRenderer + terminalsession.Frame.Render pipeline.
	views, err := composition.NativeRenderer(false)
	if err != nil {
		t.Fatalf("NativeRenderer: %v", err)
	}
	terminal, _ := views.NewAttachment(diffrender.DarkTheme)
	terminal.Width, terminal.Height = 160, 40
	renderer := nativeClientRenderer{terminal: terminal, views: views}
	now := time.Date(2026, 11, 26, 12, 0, 0, 0, time.Local)

	renderFrame := func(frame sessionapi.NativeFrame) (string, []activity.OutputDemand) {
		renderer.update(frame, conversation.DraftView{}, conversation.PromptDraftView{}, now)
		in := terminal.FrameInput(now, "0.172.0")
		if frame.Family != nil {
			in.FamilyRoot = frame.Family.Root
		}
		rendered, err := terminal.Frame.Render(ctx, in)
		if err != nil {
			t.Fatalf("Frame.Render: %v", err)
		}
		plain := make([]string, len(rendered.Rows))
		for i, r := range rendered.Rows {
			plain[i] = ansi.Strip(r)
		}
		return strings.Join(plain, "\n"), rendered.Demands
	}
	renderText := func(frame sessionapi.NativeFrame) string {
		s, demands := renderFrame(frame)
		if len(demands) > 0 {
			if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
				Output: &sessionapi.NativeOutputDemand{Demands: demands},
			}); err != nil {
				t.Fatalf("satisfy output demands: %v", err)
			}
			ev = drain()
			s, demands = renderFrame(ev.Frame)
			if len(demands) > 0 {
				t.Fatalf("unresolved output demands after hydration: %+v", demands)
			}
		}
		return s
	}

	screen := renderText(ev.Frame)
	for _, want := range []string{
		"echo hello-world",
		"main.go",
		"Done fixing regressions.",
		"$0.12",
		"total $0.48",
		"96.0 tok/s",
		"5h 88% left",
		"Weekly 79% left",
	} {
		if !strings.Contains(screen, want) {
			t.Fatalf("rendered TUI frame missing %q:\n%s", want, screen)
		}
	}
	if strings.Contains(screen, "/bin/bash -lc") {
		t.Fatalf("rendered TUI frame leaked raw shell wrapper:\n%s", screen)
	}

	// Default compact mode (Expansion == 0) clips long command output and collapses edit diffs;
	// two ctrl+e presses (Expansion == 2) expand full command output and edit diff once.
	if strings.Contains(screen, "line-1") {
		t.Fatalf("expected default compact mode to clip early output lines:\n%s", screen)
	}
	terminal.Transcript.ToggleExpansion()
	terminal.Transcript.ToggleExpansion()
	// Render once to collect the real OutputDemands emitted by terminalsession (just like nativeClient).
	_, demands := renderFrame(ev.Frame)
	if len(demands) == 0 {
		t.Fatalf("expected OutputDemands when expanding command output with ctrl+e")
	}
	_, _ = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Output: &sessionapi.NativeOutputDemand{Demands: demands},
	})
	ev = drain()
	expandedScreen := renderText(ev.Frame)
	if !strings.Contains(expandedScreen, "line-1") {
		t.Fatalf("expected ctrl+e expanded mode (Expansion == 2) to show full command output:\n%s", expandedScreen)
	}
	if !strings.Contains(expandedScreen, "Edited") || !strings.Contains(expandedScreen, "main.go") {
		t.Fatalf("expected Edited main.go in transcript:\n%s", expandedScreen)
	}
	terminal.Transcript.ToggleExpansion()

	// Open Details dialog on the commandExecution entry and hydrate DetailOutput.
	cmdSeq := conn.itemsByID["cmd-1"].seq
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Details: &sessionapi.NativeDetailRequest{
			Kind:   sessionapi.NativeEntryDetails,
			Source: cmdSeq,
		},
	}); err != nil {
		t.Fatalf("open entry details: %v", err)
	}
	ev = drain()
	if ev.Frame.Details == nil || len(ev.Frame.Details.View.Pages) == 0 {
		t.Fatalf("expected open Details dialog, got nil")
	}
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		DetailOutput: &sessionpresentation.DetailOutputRequest{
			ID:      ev.Frame.Details.View.ID,
			Page:    ev.Frame.Details.View.Pages[0].ID,
			Version: ev.Frame.Details.View.Pages[0].OutputVersion,
		},
	}); err != nil {
		t.Fatalf("detail output: %v", err)
	}
	ev = drain()
	if ev.Frame.Details.Output == nil || !slices.Contains(ev.Frame.Details.Output.View.Lines, "hello-world") {
		t.Fatalf("expected full command output in DetailOutput, got %+v", ev.Frame.Details.Output)
	}
	detailScreen := renderText(ev.Frame)
	if !strings.Contains(detailScreen, "hello-world") {
		t.Fatalf("rendered Details dialog missing command output:\n%s", detailScreen)
	}

	// Close Details, then open File details for sample.go:3.
	closeID := ev.Frame.Details.View.ID
	_, _ = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{CloseDetails: &closeID})
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Details: &sessionapi.NativeDetailRequest{
			Kind: sessionapi.NativeFileDetails,
			Path: "sample.go:3",
		},
	}); err != nil {
		t.Fatalf("open file details: %v", err)
	}
	ev = drain()
	if ev.Frame.Details == nil || ev.Frame.Details.File == nil || !strings.Contains(ev.Frame.Details.File.Source, "func Hello()") {
		t.Fatalf("expected loaded file details, got %+v", ev.Frame.Details)
	}

	// Close details and open /status panel; verify rendered panel includes account & usage.
	closeID = ev.Frame.Details.View.ID
	_, _ = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{CloseDetails: &closeID})
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/status")}); err != nil {
		t.Fatalf("/status: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Panel == nil || len(ev.Frame.View.Panel.Fields) == 0 {
		t.Fatalf("expected /status PanelView, got nil")
	}
	statusScreen := renderText(ev.Frame)
	for _, want := range []string{"gpt-5", "dev@example.com", "96.0 tok/s", "$0.12"} {
		if !strings.Contains(statusScreen, want) {
			t.Fatalf("rendered /status panel missing %q:\n%s", want, statusScreen)
		}
	}
	// Close panel via op.Panel.
	panelID := ev.Frame.View.Panel.ID
	_, _ = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Panel: &sessionpresentation.PanelAction{Panel: panelID, Kind: sessionpresentation.PanelClose},
	})
	ev = drain()
	if ev.Frame.View.Panel != nil {
		t.Fatalf("expected PanelClose to clear PanelView")
	}

	// /ps opens process dialog from thread/backgroundTerminals/list, and op.StopProcess terminates it.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/ps")}); err != nil {
		t.Fatalf("/ps: %v", err)
	}
	ev = drain()
	if len(ev.Frame.Processes) != 1 || ev.Frame.Processes[0].ID != "proc-42" || ev.Frame.Processes[0].Command != "tail -f app.log" {
		t.Fatalf("expected running background process in Frame.Processes, got %+v", ev.Frame.Processes)
	}
	if ev.Frame.Details == nil || !ev.Frame.Details.View.Processes {
		t.Fatalf("expected /ps to open Processes detail dialog")
	}
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		StopProcess: &ev.Frame.Processes[0],
	}); err != nil {
		t.Fatalf("StopProcess: %v", err)
	}
	ev = drain()
	if len(ev.Frame.Processes) != 0 {
		t.Fatalf("expected background process terminated, still have %+v", ev.Frame.Processes)
	}

	// /title, /copy, /lock, /unlock, /live, /predictions, /session, /skills, op.Reference, op.Notice, !shell restore, Frame.Live.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/title Renamed Session")}); err != nil {
		t.Fatalf("/title: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Info.Title != "Renamed Session" {
		t.Fatalf("expected updated title, got %q", ev.Frame.View.Info.Title)
	}

	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/copy")}); err != nil {
		t.Fatalf("/copy: %v", err)
	}
	ev = drain()
	foundCopy := false
	for _, eff := range ev.Frame.Effects {
		if eff.Clipboard != nil && *eff.Clipboard == "Done fixing regressions." {
			foundCopy = true
		}
	}
	if !foundCopy {
		t.Fatalf("expected /copy clipboard effect, got %+v", ev.Frame.Effects)
	}

	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/lock")}); err != nil {
		t.Fatalf("/lock: %v", err)
	}
	ev = drain()
	lastEff := ev.Frame.Effects[len(ev.Frame.Effects)-1]
	if lastEff.Presentation == nil || !lastEff.Presentation.InterruptLocked {
		t.Fatalf("expected /lock Presentation effect, got %+v", lastEff)
	}

	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Reference: &sessionpresentation.ReferenceIntent{
			Description: "main.go:1",
			Text:        "package demo",
			Source:      "main.go",
			Target:      conversation.EditorTarget{Revision: 3},
		},
	}); err != nil {
		t.Fatalf("Reference: %v", err)
	}
	ev = drain()
	lastEff = ev.Frame.Effects[len(ev.Frame.Effects)-1]
	if lastEff.Selection == nil || lastEff.Selection.Text != "package demo" {
		t.Fatalf("expected Reference Selection effect, got %+v", lastEff)
	}

	// Notice dismiss clears feedback.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Notice: &sessionapi.NativeNoticeAction{Dismiss: true},
	}); err != nil {
		t.Fatalf("Notice dismiss: %v", err)
	}
	ev = drain()
	if ev.Frame.View.Status.Feedback.Text != "" {
		t.Fatalf("expected Notice dismiss to clear feedback, got %q", ev.Frame.View.Status.Feedback.Text)
	}

	// Failed !shell emits ShellRestore effect so user draft is not lost.
	srv.mu.Lock()
	srv.shellFailMsg = "shell busy"
	srv.mu.Unlock()
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Composer: &sessionapi.NativeComposerInput{
			Draft: composer.Draft{Text: "!git status"},
		},
	}); err == nil {
		t.Fatalf("expected !shell failure error")
	}
	ev = drain()
	lastEff = ev.Frame.Effects[len(ev.Frame.Effects)-1]
	if lastEff.ShellRestore == nil || lastEff.ShellRestore.Text != "!git status" {
		t.Fatalf("expected ShellRestore effect for failed !shell, got %+v", lastEff)
	}

	// turn/diff/updated populates Frame.Live and turn/completed clears it.
	conn.handleNotification(appserver.Message{
		Method: "turn/diff/updated",
		Params: jsontext.Value(`{"threadId":"thr-main","turnId":"turn-2","diff":"diff --git a/a.go b/a.go\n--- a/a.go\n+++ b/a.go\n@@ -1 +1 @@\n-old\n+new\n"}`),
	})
	ev = drain()
	if len(ev.Frame.Live) != 1 || len(ev.Frame.Live[0].Files) != 1 || ev.Frame.Live[0].Files[0].AfterPath != "a.go" {
		t.Fatalf("expected Frame.Live from turn/diff/updated, got %+v", ev.Frame.Live)
	}
	conn.handleNotification(appserver.Message{
		Method: "turn/completed",
		Params: jsontext.Value(`{"threadId":"thr-main","turn":{"id":"turn-2","status":"completed"}}`),
	})
	ev = drain()
	if len(ev.Frame.Live) != 0 {
		t.Fatalf("expected Frame.Live cleared on turn/completed, got %+v", ev.Frame.Live)
	}
}

func TestUISnapshotDirectNativeProjection(t *testing.T) {
	socket := filepath.Join(shortTempDir(t), "native.sock")
	srv := startMockNativeAppServer(t, socket)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	conn, err := openDirectNativeConnection(ctx, socket, "/workspace/demo", nil, nil, "", "")
	if err != nil {
		t.Fatalf("openDirectNativeConnection: %v", err)
	}
	defer conn.Close()
	<-srv.ready

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
	_ = drain()

	now := time.Date(2026, 11, 26, 12, 0, 0, 0, time.Local)
	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{
			"threadId":"thr-main","turnId":"turn-1",
			"item":{
				"type":"commandExecution","id":"cmd-1",
				"command":"/bin/bash -lc 'go test ./internal/launch'",
				"status":"completed","exitCode":0,
				"aggregatedOutput":"ok 1\nok 2\nok 3\nok 4\nok 5\nPASS\n"
			}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{
			"threadId":"thr-main","turnId":"turn-1",
			"item":{
				"type":"fileChange","id":"fc-1","status":"completed",
				"changes":[{"path":"main.go","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-old\n+new\n"}]
			}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "item/completed",
		Params: jsontext.Value(`{
			"threadId":"thr-main","turnId":"turn-1",
			"item":{"type":"agentMessage","id":"msg-1","text":"Fixed native TUI projection."}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "thread/tokenUsage/updated",
		Params: jsontext.Value(`{
			"threadId":"thr-main",
			"tokenUsage":{
				"total":{"totalTokens":15000,"inputTokens":12000,"outputTokens":3000,"cachedInputTokens":1000,"reasoningOutputTokens":200},
				"last":{"totalTokens":100000},
				"modelContextWindow":400000,
				"estimatedCostUsd":0.12,
				"costPartial":false,
				"outputTokensPerSecond":96.0,
				"workDurationMs":1500,
				"roundtrips":2
			}
		}`),
	})
	conn.handleNotification(appserver.Message{
		Method: "thread/tokenUsage/updated",
		Params: jsontext.Value(`{
			"threadId":"thr-worker",
			"tokenUsage":{
				"total":{"totalTokens":5000,"inputTokens":4000,"outputTokens":1000,"cachedInputTokens":0,"reasoningOutputTokens":0},
				"last":{"totalTokens":10000},
				"modelContextWindow":200000,
				"estimatedCostUsd":0.36,
				"costPartial":false,
				"outputTokensPerSecond":64.0,
				"workDurationMs":800,
				"roundtrips":1
			}
		}`),
	})
	// Pin entry timestamps for deterministic snapshots.
	conn.mu.Lock()
	for _, rec := range conn.items["thr-main"] {
		rec.entry.Observed = now
		if !rec.entry.Completed.IsZero() {
			rec.entry.Completed = now
		}
	}
	conn.publishLocked()
	conn.mu.Unlock()
	ev := drain()

	views, err := composition.NativeRenderer(false)
	if err != nil {
		t.Fatalf("NativeRenderer: %v", err)
	}
	terminal, _ := views.NewAttachment(diffrender.DarkTheme)
	terminal.Width, terminal.Height = 140, 24
	renderer := nativeClientRenderer{terminal: terminal, views: views}

	renderFrame := func(frame sessionapi.NativeFrame) string {
		renderer.update(frame, conversation.DraftView{}, conversation.PromptDraftView{}, now)
		in := terminal.FrameInput(now, "0.172.0")
		if frame.Family != nil {
			in.FamilyRoot = frame.Family.Root
		}
		rendered, err := terminal.Frame.Render(ctx, in)
		if err != nil {
			t.Fatalf("Frame.Render: %v", err)
		}
		if len(rendered.Demands) > 0 {
			if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
				Output: &sessionapi.NativeOutputDemand{Demands: rendered.Demands},
			}); err != nil {
				t.Fatalf("satisfy output demands: %v", err)
			}
			ev = drain()
			renderer.update(ev.Frame, conversation.DraftView{}, conversation.PromptDraftView{}, now)
			in = terminal.FrameInput(now, "0.172.0")
			if ev.Frame.Family != nil {
				in.FamilyRoot = ev.Frame.Family.Root
			}
			rendered, err = terminal.Frame.Render(ctx, in)
			if err != nil {
				t.Fatalf("Frame.Render after hydration: %v", err)
			}
		}
		return strings.Join(rendered.Rows, "\n")
	}

	// 1. Default compact transcript + caption with cost, tok/s, and rate limits.
	uisnapshot.Assert(t, "testdata/snapshots/direct-native-compact-140.txt", renderFrame(ev.Frame))

	// 2. ctrl+e expanded view (Expansion == 2) with full hydrated command output.
	terminal.Transcript.ToggleExpansion()
	terminal.Transcript.ToggleExpansion()
	uisnapshot.Assert(t, "testdata/snapshots/direct-native-expanded-140.txt", renderFrame(ev.Frame))
	terminal.Transcript.ToggleExpansion()

	// 3. Entry Details dialog with hydrated DetailOutput.
	cmdSeq := conn.itemsByID["cmd-1"].seq
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		Details: &sessionapi.NativeDetailRequest{Kind: sessionapi.NativeEntryDetails, Source: cmdSeq},
	}); err != nil {
		t.Fatalf("open details: %v", err)
	}
	ev = drain()
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{
		DetailOutput: &sessionpresentation.DetailOutputRequest{
			ID:      ev.Frame.Details.View.ID,
			Page:    ev.Frame.Details.View.Pages[0].ID,
			Version: ev.Frame.Details.View.Pages[0].OutputVersion,
		},
	}); err != nil {
		t.Fatalf("detail output: %v", err)
	}
	ev = drain()
	uisnapshot.Assert(t, "testdata/snapshots/direct-native-details-140.txt", renderFrame(ev.Frame))
	closeID := ev.Frame.Details.View.ID
	_, _ = conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{CloseDetails: &closeID})

	// 4. /status panel.
	if _, err := conn.Send(ctx, sessionapi.NativeTarget{}, sessionapi.NativeOperation{Command: new("/status")}); err != nil {
		t.Fatalf("/status: %v", err)
	}
	ev = drain()
	uisnapshot.Assert(t, "testdata/snapshots/direct-native-status-140.txt", renderFrame(ev.Frame))
}
