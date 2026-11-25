package launch

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/moduletest"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func TestUISnapshotCompactionSummaryDialog(t *testing.T) {
	const summary = "## Current work\n\nKeep the **first** boundary's exact summary.\n\n```go\nreturn saved\n```\n"
	p := newManagedMekugiProxy(t)
	attachTestReplayStore(t, p)
	first, err := moduletest.StoreCompact(t.Context(), p.retained, "main", summary, `{"sources":["@never-read.txt"]}`)
	if err != nil {
		t.Fatal(err)
	}
	second, err := moduletest.StoreCompact(t.Context(), p.retained, "main", "A different later summary.", "")
	if err != nil {
		t.Fatal(err)
	}
	// A local fork can retain copied ancestor metadata before its own boundaries.
	records := []map[string]any{replayTestMeta("main"), replayTestMeta("ancestor")}
	for i, ref := range []string{first, second} {
		turn, item := []string{"one", "two"}[i], []string{"first", "second"}[i]
		records = append(records,
			replayTestRecord("event_msg", replayTestEpoch+int64(i)*3000, map[string]any{"type": "task_started", "turn_id": turn}),
			replayTestRecord("compacted", replayTestEpoch+int64(i)*3000+500, map[string]any{"replacement_history": []any{map[string]any{"type": "compaction", "encrypted_content": ref}}}),
			replayTestItem("main", turn, replayTestEpoch+int64(i)*3000+100, replayTestEpoch+int64(i)*3000+1000, map[string]any{"id": item, "type": "ContextCompaction"}),
			replayTestRecord("event_msg", replayTestEpoch+int64(i)*3000+1500, map[string]any{"type": "task_complete", "turn_id": turn}),
		)
	}
	path := replayTestWrite(t, t.TempDir(), "rollout-main.jsonl", records...)
	for _, mode := range []string{"live", "replacement", "restore"} {
		t.Run(mode, func(t *testing.T) {
			var u *appServerUI
			if mode == "replacement" {
				var wire *appServerTestInput
				u, wire = newResumeSessionTestUI(t)
				u.retained = p.retained
				u.retainedSession = history.Session{Thread: "old", Context: t.Context()}
				hooks := u.attached.hooks
				appServerTestKeys(t, u, "/clear\r")
				start := resumeTestOne(t, wire, "thread/start")
				appServerTestMessage(t, u, fmt.Sprintf(`{"id":%d,"result":{"thread":{"id":"main","cwd":"/work","path":%q}}}`, start.ID, path))
				resumeTestSettle(t, u, wire)
				if u.attached.hooks != hooks || u.retainedSession.Thread != "main" {
					t.Fatal("replacement lost the hooks or current retained session")
				}
			} else {
				u = newAppServerSessionTestUI(t, t.TempDir())
				u.retainedSession = history.Session{Store: p.retained.Store, Thread: "main", Context: t.Context()}
				u.RolloutPath = path
			}
			if mode == "live" || mode == "replacement" {
				for i, id := range []string{"first", "second"} {
					appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": []string{"one", "two"}[i], "item": binding.ThreadItem{ID: id, Type: "contextCompaction", CompactionMethod: observation.ContextSummary}})
				}
			} else if mode == "restore" {
				info := binding.ThreadInfo{ID: "main", Path: path, Cwd: u.attached.cwd, Turns: []binding.HistoryTurn{
					{ID: "one", Status: "completed", Items: []binding.ThreadItem{{ID: "first", Type: "contextCompaction"}}},
					{ID: "two", Status: "completed", Items: []binding.ThreadItem{{ID: "second", Type: "contextCompaction"}}},
				}}
				if err := u.restoreContent(info); err != nil {
					t.Fatal(err)
				}
			}
			_, view, _ := nativeDetailTestViews(t, u)
			renderer := nativeDetailTestRenderer(t, u, view)
			feed := renderer.terminal.Transcript.RenderFeed(80, 20)
			for _, snippet := range feed.Snippets {
				block, ok := renderer.terminal.Transcript.SnippetBlock(snippet)
				if !ok || block.Kind != "compaction" || block.Source != view.attached.source.Entries[0].Source.Seq {
					continue
				}
				if failure := view.openDetails(sessionapi.NativeDetailRequest{Kind: sessionapi.NativeEntryDetails, Source: block.Source}); failure != "" {
					t.Fatal(failure)
				}
				if mode == "live" || mode == "replacement" {
					result := <-view.files.fileReads
					view.files.loadedMarkdownFile(&view.attached.session, view.attached.presentation, view.attached.cwd, result)
				}
				renderer.updateNativeDetailTest(view)
				rows := make([]string, 20)
				renderer.terminal.Dialog.Paint(rows, terminalsession.Rect{W: 80, H: 20})
				if got := renderer.terminal.Dialog.Document.Text; got != summary {
					t.Fatalf("%s summary=%q", mode, got)
				}
				if mode == "live" {
					uisnapshot.AssertTerminal(t, "testdata/snapshots/compaction-summary-dialog.txt", append(rows, "plain after dialog"), 80)
				}
				// An open dialog keeps its selected summary after feed aging.
				view.attached.source.Entries = nil
				renderer.updateNativeDetailTest(view)
				renderer.terminal.Dialog.Paint(make([]string, 20), terminalsession.Rect{W: 80, H: 20})
				if renderer.terminal.Dialog.Document.Text != summary {
					t.Fatal("feed aging changed the open summary")
				}
				return
			}
			t.Fatalf("summary has no clickable target: %q", feed.Lines)
		})
	}
	foreign, err := moduletest.StoreCompact(t.Context(), p.retained, "foreign", "foreign summary", "")
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := history.ReadCompactSummary(p.retained.Store, "main", foreign); err == nil {
		t.Fatal("foreign summary was admitted")
	}
}

func TestCompactionUnsuccessfulCompletionHasNoSuccessRow(t *testing.T) {
	for _, mode := range []string{"main", "child", "restore"} {
		t.Run(mode, func(t *testing.T) {
			u := newAppServerSessionTestUI(t, t.TempDir())
			items := []binding.ThreadItem{
				{ID: "success", Type: "contextCompaction", Status: "completed"},
				{ID: "failure", Type: "contextCompaction", Status: "failed"},
				{ID: "interruption", Type: "contextCompaction", Status: "interrupted"},
			}
			view := u.testActivity(u.shell.Transcript).View
			if mode == "restore" {
				if err := u.restoreContent(binding.ThreadInfo{ID: "main", Cwd: u.attached.cwd, Turns: []binding.HistoryTurn{{ID: "turn", Status: "completed", Items: items}}}); err != nil {
					t.Fatal(err)
				}
			} else {
				if mode == "child" {
					appServerTestNotify(t, u, "thread/started", map[string]any{"thread": map[string]any{"id": "child", "agentNickname": "worker"}})
					child := u.testActivity(u.shell.Agents)
					child.setDisplayFilter(false, "/root/worker")
					view = child.View
				}
				for _, item := range items {
					appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": mode, "turnId": "turn", "item": item})
				}
			}
			u.testActivity(view).publishActivity()
			label := "Context reset"
			if mode == "restore" {
				label = "Context compacted"
			}
			if rendered := strings.Join(view.RenderConversation(80).Lines, "\n"); strings.Count(rendered, label) != 1 {
				t.Fatalf("successful boundary was not rendered exactly once: %s", rendered)
			}
		})
	}
}

func TestCompactionSummaryReadLeavesInputResponsiveAndRejectsStaleResult(t *testing.T) {
	u := newAppServerSessionTestUI(t, t.TempDir())
	started, release, done := make(chan struct{}), make(chan struct{}), make(chan struct{})
	u.attached.hooks.compaction = func(thread, item string) func(context.Context) (string, error) {
		return func(ctx context.Context) (string, error) {
			close(started)
			defer close(done)
			select {
			case <-release:
			case <-time.After(time.Second):
			}
			return "late summary", nil
		}
	}
	appServerTestNotify(t, u, "item/completed", map[string]any{"threadId": "main", "turnId": "one", "item": binding.ThreadItem{ID: "boundary", Type: "contextCompaction", CompactionMethod: observation.ContextSummary}})
	_, view, _ := nativeDetailTestViews(t, u)
	seq := view.attached.source.Entries[0].Source.Seq
	if failure := view.openDetails(sessionapi.NativeDetailRequest{Kind: sessionapi.NativeEntryDetails, Source: seq}); failure != "" {
		t.Fatal(failure)
	}
	<-started
	select {
	case <-done:
		t.Fatal("dialog opening waited for storage")
	default:
	}
	u.applyComposerKey("x")
	appServerTestNotify(t, u, "turn/started", map[string]any{"threadId": "main", "turn": map[string]any{"id": "responsive"}})
	if u.Turn != "responsive" || u.attached.conversation.Editor.Text != "x" {
		t.Fatal("input/lifecycle did not proceed while storage was blocked")
	}
	old := view.files.fileReads
	view.openNoticeDetails("replacement dialog")
	close(release)
	result := <-old
	if changed, _ := view.files.loadedMarkdownFile(&view.attached.session, view.attached.presentation, view.attached.cwd, result); changed {
		t.Fatal("late summary replaced another dialog")
	}
	renderer := nativeDetailTestRenderer(t, u, view)
	renderer.terminal.Dialog.Paint(make([]string, 20), terminalsession.Rect{W: 80, H: 20})
	if renderer.terminal.Dialog.Document.Text != "replacement dialog" {
		t.Fatal("replacement dialog changed")
	}
}
