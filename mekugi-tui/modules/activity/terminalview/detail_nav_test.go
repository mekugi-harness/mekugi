package terminalview

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
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

	// Initially following at bottom: first edit (a.go / b.go) is scrolled above the 6-row viewport.
	if v.Offset == 0 {
		t.Fatalf("expected viewport scrolled to bottom, got Offset=0 (FeedLines=%d)", v.FeedLines)
	}

	// Activating a.go scrolls up to reveal it and highlights only a.go's gutter with file-gutter palette.
	v.SetActiveDetail(first, true)
	rows := v.Render(80, 6, time.Unix(1, 0))
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
