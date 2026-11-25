package terminalview

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/vt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func TestDetailTargetsAndActiveGutterHighlight(t *testing.T) {
	v := NewView()
	v.Conversation = true
	v.FeedOnly = true
	v.Painter.Theme = diffrender.DarkTheme

	entries := []activity.Entry{
		{
			Seq: 1, Agent: "Main", Kind: "tool",
			Values: []activity.Value{
				{Source: 1, Kind: "op", Verb: "Edit", Label: "`a.go`", Path: "a.go", Effect: true},
				{Source: 1, Kind: "op", Verb: "Edit", Label: "`b.go`", Path: "b.go", Effect: true},
			},
		},
		{
			Seq: 2, Agent: "Main", Kind: "text",
			Text: strings.Repeat("Spacer message line.\n", 14),
		},
		{
			Seq: 3, Agent: "Main", Kind: "tool",
			Values: []activity.Value{
				{Source: 3, Kind: "op", Verb: "Edit", Label: "`c.go`", Path: "c.go", Effect: true},
				{Source: 3, Kind: "op", Verb: "Edit", Label: "`d.go`", Path: "d.go", Effect: true},
			},
		},
	}
	v.Update(&activity.Publication{Entries: entries}, Display{Caller: "/root"})
	v.Follow()
	_ = v.Render(80, 6, time.Unix(1, 0))
	if len(v.FeedTargets) != 0 {
		t.Fatalf("expected no FeedTargets work while no detail sheet is open, got %d", len(v.FeedTargets))
	}

	// Activating a.go scrolls up to reveal it, populates FeedTargets, and highlights only a.go's gutter.
	first := DetailTarget{Snippet: Snippet{Run: 1, Block: EditNavigationSnippet, Path: "a.go"}}
	v.SetActiveDetail(first, true)
	rows := v.Render(80, 6, time.Unix(1, 0))

	if len(v.FeedTargets) != 2 {
		t.Fatalf("got %d targets, want 2 (skipping unclickable message): %+v", len(v.FeedTargets), v.FeedTargets)
	}
	first, second := v.FeedTargets[0], v.FeedTargets[1]
	if first.Snippet.Run != 1 || first.Snippet.Block != EditNavigationSnippet || first.Snippet.Path != "a.go" || first.LastSnippet.Path != "b.go" {
		t.Fatalf("unexpected first target: %+v", first)
	}
	if second.Snippet.Run != 3 || second.Snippet.Block != EditNavigationSnippet || second.Snippet.Path != "c.go" || second.LastSnippet.Path != "d.go" {
		t.Fatalf("unexpected second target: %+v", second)
	}

	wantStyle := v.Painter.Theme.Accent() + v.Painter.Theme.SelectionBackground()
	var aRow, bRow string
	for _, row := range rows {
		plain := ansi.Strip(row)
		if strings.Contains(plain, "a.go") {
			aRow = row
		}
		if strings.Contains(plain, "b.go") {
			bRow = row
		}
	}
	if aRow == "" || bRow == "" {
		t.Fatalf("a.go/b.go not scrolled into view:\n%s", strings.Join(rows, "\n"))
	}
	if !strings.HasPrefix(aRow, wantStyle) {
		t.Fatalf("a.go gutter not highlighted: %q", aRow)
	}
	if strings.HasPrefix(bRow, wantStyle) {
		t.Fatalf("b.go gutter should not be highlighted while a.go is active: %q", bRow)
	}
	if v.Following {
		t.Fatal("expected Following=false while revealing offscreen active event")
	}
	for _, row := range rows {
		if strings.Contains(ansi.Strip(row), "Back to bottom") {
			t.Fatalf("Back to bottom hint should stay hidden while active detail is open: %q", row)
		}
	}

	// Moving active detail to b.go shifts the gutter highlight to b.go.
	bTarget := first
	bTarget.Snippet = first.LastSnippet
	v.SetActiveDetail(bTarget, true)
	rows = v.Render(80, 6, time.Unix(1, 0))
	aRow, bRow = "", ""
	for _, row := range rows {
		plain := ansi.Strip(row)
		if strings.Contains(plain, "a.go") {
			aRow = row
		}
		if strings.Contains(plain, "b.go") {
			bRow = row
		}
	}
	if strings.HasPrefix(aRow, wantStyle) || !strings.HasPrefix(bRow, wantStyle) {
		t.Fatalf("expected highlight on b.go only:\na=%q\nb=%q", aRow, bRow)
	}

	// Clearing active detail removes the gutter highlight.
	v.ClearActiveDetail()
	rows = v.Render(80, 6, time.Unix(1, 0))
	for i, row := range rows {
		if strings.Contains(row, wantStyle) {
			t.Fatalf("row %d retained gutter highlight after clear: %q", i, row)
		}
	}
}

func TestHighlightTranscriptGutterPreservesTrailingStyle(t *testing.T) {
	theme := diffrender.DarkTheme
	line := "\x1b[2m• Thought for 3s\x1b[m"
	got := highlightTranscriptGutter(line, theme)
	wantPrefix := theme.Accent() + theme.SelectionBackground() + "• " + terminalui.Reset + "\x1b[2m"
	if !strings.HasPrefix(got, wantPrefix) {
		t.Fatalf("got %q, want prefix %q", got, wantPrefix)
	}
	if ansi.Strip(got) != ansi.Strip(line) {
		t.Fatalf("plain text changed: got %q want %q", ansi.Strip(got), ansi.Strip(line))
	}
	_ = fmt.Sprint(got)
}

func TestDetailTargetsCollapseMultiSegmentCommand(t *testing.T) {
	v := NewView()
	v.Conversation = true
	v.FeedOnly = true
	entries := []activity.Entry{
		{
			Seq: 1, Agent: "Main", Kind: "command",
			Values: []activity.Value{
				{Source: 1, Operation: 1, Output: 1, Kind: "op", Verb: "Run", Code: "echo one"},
				{Source: 1, Operation: 2, Output: 2, Kind: "op", Verb: "Run", Code: "echo two"},
				{Source: 1, Kind: "op", Verb: "Edit", Label: "`a.go`", Path: "a.go", Effect: true},
			},
		},
		{
			Seq: 2, Agent: "Main", Kind: "command",
			Values: []activity.Value{
				{Source: 2, Operation: 1, Output: 1, Kind: "op", Verb: "Run", Code: "echo three"},
			},
		},
	}
	v.Update(&activity.Publication{Entries: entries}, Display{Caller: "/root"})
	v.SetActiveDetail(DetailTarget{Snippet: Snippet{Run: 1, Block: 0}}, true)
	_ = v.Render(80, 10, time.Unix(1, 0))

	// Two segments from Source=1 collapse into one command target, followed by the Source=1 edit and Source=2 command.
	if len(v.FeedTargets) != 3 {
		t.Fatalf("got %d targets, want 3 (collapsed multi-segment command, edit, next command): %+v", len(v.FeedTargets), v.FeedTargets)
	}
	if v.FeedTargets[0].Source != 1 || v.FeedTargets[0].Edit {
		t.Fatalf("target 0 = %+v, want non-edit Source=1", v.FeedTargets[0])
	}
	if v.FeedTargets[1].Source != 1 || !v.FeedTargets[1].Edit {
		t.Fatalf("target 1 = %+v, want edit Source=1", v.FeedTargets[1])
	}
	if v.FeedTargets[2].Source != 2 || v.FeedTargets[2].Edit {
		t.Fatalf("target 2 = %+v, want non-edit Source=2", v.FeedTargets[2])
	}
}

func TestActiveDetailScrollsBelowPinnedHeading(t *testing.T) {
	v := NewView()
	v.Conversation = true
	v.FeedOnly = true
	entries := []activity.Entry{
		{
			Seq: 1, Agent: "Main", Kind: "tool",
			Values: []activity.Value{
				{Source: 1, Kind: "op", Verb: "Edit", Workdir: "sub", Label: "`sub/a.go`", Path: "sub/a.go", Effect: true},
				{Source: 2, Kind: "op", Verb: "Edit", Workdir: "sub", Label: "`sub/b.go`", Path: "sub/b.go", Effect: true},
				{Source: 3, Kind: "op", Verb: "Edit", Workdir: "sub", Label: "`sub/c.go`", Path: "sub/c.go", Effect: true},
				{Source: 4, Kind: "op", Verb: "Edit", Workdir: "sub", Label: "`sub/d.go`", Path: "sub/d.go", Effect: true},
			},
		},
	}
	v.Update(&activity.Publication{Entries: entries}, Display{Caller: "/root"})
	// Place viewport offset right on sub/a.go (row 1, directly below the workdir heading at row 0)
	// and verify PinnedHeading actually covers sub/a.go before reveal scrolls back to row 0.
	v.Following = false
	v.Offset = 1
	before := ansi.Strip(strings.Join(v.Render(80, 3, time.Unix(1, 0)), "\n"))
	if !strings.Contains(before, "sub") || strings.Contains(before, "a.go") {
		t.Fatalf("expected pinned workdir heading to cover a.go at offset 1, got:\n%s", before)
	}
	v.SetActiveDetail(DetailTarget{Snippet: Snippet{Run: 1, Block: EditNavigationSnippet, Path: "sub/a.go"}}, true)
	rows := v.Render(80, 3, time.Unix(1, 0))
	joined := ansi.Strip(strings.Join(rows, "\n"))
	if !strings.Contains(joined, "a.go") {
		t.Fatalf("active edit was hidden by pinned workdir heading:\n%s", joined)
	}
}

func TestActiveDetailRevealKeepsClickedCommandSegmentVisible(t *testing.T) {
	v := NewView()
	v.Conversation = true
	v.FeedOnly = true
	entries := []activity.Entry{
		{
			Seq: 1, Agent: "Main", Kind: "command",
			Values: []activity.Value{
				{
					Source: 1, Operation: 1, Output: 1, Kind: "op", Verb: "Run",
					Code: "echo first-segment",
					Tail: []string{"line 1", "line 2", "line 3", "line 4", "line 5", "line 6"},
				},
				{
					Source: 1, Operation: 2, Output: 2, Kind: "op", Verb: "Run",
					Code: "echo second-segment",
					Tail: []string{"second output"},
				},
			},
		},
	}
	v.Update(&activity.Publication{Entries: entries}, Display{Caller: "/root"})
	v.Follow()
	_ = v.Render(80, 6, time.Unix(1, 0))

	// Open the second segment (block 1) of the multi-segment command; reveal must keep it visible
	// rather than scrolling back to block 0 when the full invocation exceeds viewport height.
	v.SetActiveDetail(DetailTarget{Snippet: Snippet{Run: 1, Block: 1}}, true)
	rows := v.Render(80, 6, time.Unix(1, 0))
	joined := ansi.Strip(strings.Join(rows, "\n"))
	if !strings.Contains(joined, "second-segment") {
		t.Fatalf("clicked second command segment was scrolled out of view:\n%s", joined)
	}
	if len(v.FeedTargets) != 1 || v.FeedTargets[0].Source != 1 {
		t.Fatalf("expected multi-segment command to stay collapsed to 1 target, got %+v", v.FeedTargets)
	}
}

func TestDetailRunIndexResolvesManyRunsInSinglePass(t *testing.T) {
	v := NewView()
	v.Conversation = true
	v.FeedOnly = true
	const total = 150
	entries := make([]activity.Entry, 0, total)
	for i := uint64(1); i <= total; i++ {
		entries = append(entries, activity.Entry{
			Seq: i, Agent: "Main", Kind: "command",
			Values: []activity.Value{
				{Source: i, Operation: 1, Output: 1, Kind: "op", Verb: "Run", Code: fmt.Sprintf("echo cmd-%d", i)},
			},
		})
	}
	v.Update(&activity.Publication{Entries: entries}, Display{Caller: "/root"})
	v.Follow()
	v.SetActiveDetail(DetailTarget{Snippet: Snippet{Run: total, Block: 0}}, true)
	rows := v.Render(80, 6, time.Unix(1, 0))
	if len(v.FeedTargets) != total {
		t.Fatalf("got %d targets, want %d", len(v.FeedTargets), total)
	}
	joined := ansi.Strip(strings.Join(rows, "\n"))
	if !strings.Contains(joined, fmt.Sprintf("cmd-%d", total)) {
		t.Fatalf("expected active last command visible:\n%s", joined)
	}
}

func TestUISnapshotTranscriptGutterHighlight(t *testing.T) {
	for _, theme := range []diffrender.Theme{diffrender.TerminalTheme, diffrender.DarkTheme, diffrender.LightTheme} {
		t.Run(fmt.Sprint(theme), func(t *testing.T) {
			const width = 60
			v := NewView()
			v.Conversation = true
			v.FeedOnly = true
			v.Painter.Theme = theme
			entries := []activity.Entry{
				{
					Seq: 1, Agent: "Main", Kind: "tool",
					Values: []activity.Value{
						{Source: 1, Kind: "op", Verb: "Edit", Label: "`a.go`", Path: "a.go", Effect: true},
						{Source: 1, Kind: "op", Verb: "Edit", Label: "`b.go`", Path: "b.go", Effect: true},
					},
				},
			}
			v.Update(&activity.Publication{Entries: entries}, Display{Caller: "/root"})
			v.SetActiveDetail(DetailTarget{Snippet: Snippet{Run: 1, Block: EditNavigationSnippet, Path: "a.go"}}, true)
			rows := v.Render(width, 3, time.Unix(1, 0))
			rows = append(rows, "plain after transcript gutter")
			screen := vt.NewEmulator(width, len(rows))
			defer screen.Close()
			if _, err := screen.Write([]byte(strings.Join(rows, "\r\n"))); err != nil {
				t.Fatal(err)
			}
			painted := strings.Split(screen.Render(), "\n")
			uisnapshot.AssertTerminal(t, fmt.Sprintf("testdata/snapshots/transcript-gutter-highlight-%d.txt", theme), painted, width)
		})
	}
}
