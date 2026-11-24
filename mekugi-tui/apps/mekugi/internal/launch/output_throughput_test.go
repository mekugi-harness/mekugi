package launch

import (
	"fmt"
	"strings"
	"testing"
	"testing/synctest"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
)

func TestUISnapshotOutputThroughputComposerAndRoster(t *testing.T) {
	now := time.Date(2026, 9, 30, 12, 0, 0, 0, time.UTC)
	for _, width := range []int{36, 80, 140} {
		t.Run(fmt.Sprint(width), func(t *testing.T) {
			u, _ := newAppServerTestUI()
			u.startSession(u.Thread, "")

			u.shell.Transcript.Painter.Theme = diffrender.DarkTheme
			root := u.attached.session.Agent("/root")
			root.ContextKnown, root.ContextTokens, root.ContextWindow = true, 24000, 200000
			root.OutputThroughput = inference.OutputThroughput{OutputTokens: 83, DurationNanos: uint64(2 * time.Second), MeasuredRequests: 1}
			u.attached.session.Lifecycle(u.Thread).Final = true
			u.attached.presentation.Status.Text, u.settings.Model = "Ready", "gpt-6-sol"
			rows, _ := u.testMainFrame(width, 10, 0)
			assertNativeUISnapshot(t, fmt.Sprintf("output-throughput-composer-%d", width), rows)
			u.testActivity(u.shell.Agents).Apply(sessionpresentation.ActivityEvent{Agents: []sessionpresentation.ActivityAgent{*root,
				{Name: "/root/fast", OutputThroughput: inference.OutputThroughput{OutputTokens: 120, DurationNanos: uint64(time.Second), MeasuredRequests: 1}, ContextKnown: true, ContextTokens: 1000, ContextWindow: 200000},
				{Name: "/root/zero", OutputThroughput: inference.OutputThroughput{DurationNanos: uint64(time.Second), MeasuredRequests: 1}},
				{Name: "/root/absent"},
			}})
			assertNativeUISnapshot(t, fmt.Sprintf("output-throughput-roster-%d", width), u.testActivity(u.shell.Agents).RosterSheet(width, 6, now))
		})
	}
}

func TestUISnapshotOutputThroughputRestoredRoster(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		store := usageStoreFixture(t, t.TempDir())
		writeLegacyUsageFixture(t, store.directory, "child", `"Counts":{"OutputTokens":100},"OutputCost":0.001,
			"CostKnown":true,"Complete":true,"Roundtrips":1,"RoundOutput":{"Throughput":{"output_tokens":100,"duration_ns":2000000000,"measured_requests":1}}`)
		u := newAppServerSessionTestUI(t, t.TempDir())
		setTestAppServerIntegration(u, &observationFixture{usage: storedUsageFixture(t, store)})

		for _, thread := range []string{"child", "interrupted", "legacy"} {
			info := binding.ThreadInfo{ID: thread, AgentNickname: thread, Turns: []binding.HistoryTurn{{ID: "old", Status: "completed"}}}
			registerThread(&u.attached.session, info)
			restoreUsage(u.usage, info)
			observeCost(u.usage, thread, u.attached.session.Agent(u.attached.session.KnownCaller(thread)))
			u.attached.session.Lifecycle(thread).Restore(restoredAgentIntervals(info.Turns), uint64(len(info.Turns)), false)
			u.attached.session.Lifecycle(thread).Final = true
		}
		u.testActivity(u.shell.Agents).Apply(sessionpresentation.ActivityEvent{Agents: u.attached.session.Roster()})
		u.shell.Agents.Painter.Theme = diffrender.DarkTheme
		u.shell.Agents.Selected = u.attached.session.KnownCaller("child")
		assertNativeUISnapshot(t, "output-throughput-restored-roster", u.testActivity(u.shell.Agents).RosterSheet(120, 8, time.Now()))
	})
}

func TestOutputThroughputPresentationSelectionAndIsolation(t *testing.T) {
	for _, mode := range []string{"all", "without usage", "without activity"} {
		t.Run(mode, func(t *testing.T) {
			u := newAppServerSessionTestUI(t, t.TempDir())
			if mode == "without usage" {
				u.presentationState = new(sessionpresentation.Service)
				u.attached.presentation = u.presentationState.Session(u.attached.session.Scope())
				attachPresentation(&u.attached, &u.picker, &u.Editor, &u.questions, &u.approvals, u.Thread, &u.review, &u.commits, u.ctx, u.retained, u.changeSource, u.activity != nil)
			}
			if mode == "without activity" {
				u.activity = nil
			}

			u.settings.Model = "main-model"
			rate := inference.OutputThroughput{OutputTokens: 40, DurationNanos: uint64(time.Second), MeasuredRequests: 1}
			u.attached.session.Agent("/root").OutputThroughput = rate
			before := publishTestPresentation(&u.attached, u.shell, &u.Client, &u.picker, &u.Editor, u.settings, u.title, u.backendVersion, u.restoring != nil, u.side)
			caption, _ := u.testStatLines(t, 120, 12)
			if text := ansi.Strip(caption); !strings.Contains(text, "main-model") || strings.Contains(text, "40.0 tok/s") != (mode != "without usage") {
				t.Fatalf("Main contribution: %s", text)
			}
			registerThread(&u.attached.session, binding.ThreadInfo{ID: "child", AgentNickname: "child"})
			child := u.attached.session.Agent("/root/child")
			rate.OutputTokens = 80
			child.OutputThroughput = rate
			u.testActivity(u.shell.Agents).Apply(sessionpresentation.ActivityEvent{Agents: u.attached.session.Roster()})
			roster := ansi.Strip(strings.Join(u.testActivity(u.shell.Agents).RosterSheet(140, 8, presentationTime(u.clock)), "\n"))
			if strings.Contains(roster, "80.0 tok/s") != (mode != "without usage") {
				t.Fatalf("roster contribution: %s", roster)
			}
			u.testActivity(u.shell.Agents).displayThread("/root/child")
			caption, _ = u.testStatLines(t, 120, 12)
			if text := ansi.Strip(caption); strings.Contains(text, "40.0 tok/s") || strings.Contains(text, "80.0 tok/s") != (mode != "without usage") {
				t.Fatalf("child contribution: %s", text)
			}
			if mode != "without usage" && (len(before.Caption.Items) != 1 || before.Caption.Items[0].Thread != "main" || before.Caption.Items[0].Text != "40.0 tok/s") {
				t.Fatalf("earlier publication changed: %+v", before.Caption.Items)
			}
		})
	}
}

func TestOutputThroughputPresentationReplayReset(t *testing.T) {
	p := newTestUIReplayPlayback(nil, t.Context(), replayPlaybackTestSource(), 1)
	defer p.close()
	if err := p.seek(p.Until); err != nil {
		t.Fatal(err)
	}
	p.ui.session.Agent("/root").OutputThroughput = inference.OutputThroughput{OutputTokens: 40, DurationNanos: uint64(time.Second), MeasuredRequests: 1}
	p.publish()
	before := p.ui.presentation.View()
	if len(before.Caption.Items) != 1 {
		t.Fatal("admitted replay fact was not published")
	}
	if err := p.seek(0); err != nil {
		t.Fatal(err)
	}
	p.publish()
	after := p.ui.presentation.View()
	if len(after.Caption.Items) != 0 || before.Version.Attachment == after.Version.Attachment {
		t.Fatalf("backward replay retained future facts: %+v", after)
	}
}
