package terminalview_test

import (
	"fmt"
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/terminalview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/uisnapshot"
)

func previewDock() *prompt.Questions {
	c := prompt.Call{
		ID: "preview", Thread: "main",
		Questions: []prompt.Question{{
			ID: "q1", Question: "How should we publish the release?", Selected: 1,
			Choices: []prompt.Option{
				{Label: "Immediate", Preview: "Publish all changes now."},
				{Label: "Staged", Recommended: true, Preview: "Release plan\n\n1. Publish to the canary group.\n2. Monitor errors for one hour.\n3. Expand to all customers.\n\n<b>Plain text, not HTML.</b>"},
				{Label: "Later", Description: "Keep the current version"},
			},
		}},
	}
	var questions prompt.Questions
	questions.Calls = append(questions.Calls, &c)
	questions.Open(&c)
	return &questions
}

func previewRows(m *prompt.Questions, layout *terminalview.QuestionLayout, width, height int) []string {
	return terminalview.Questions(m.View("main"), &terminalui.Painter{}, layout, "", false, width, height)
}

func TestQuestionPreviewFocusScrollAndClear(t *testing.T) {
	m := previewDock()
	m.Current().Selected = 0
	var lines []string
	for i := 1; i <= 40; i++ {
		lines = append(lines, fmt.Sprintf("preview line %02d", i))
	}
	m.Current().Choices[0].Preview = strings.Join(lines, "\n")
	var layout terminalview.QuestionLayout
	paint := func(width int) string {
		rows := previewRows(m, &layout, width, 14)
		for _, row := range rows {
			if ansi.StringWidth(row) > width {
				t.Fatal("preview exceeded width", row)
			}
		}
		if len(rows) > 14 {
			t.Fatal("preview exceeded height")
		}
		return ansi.Strip(strings.Join(rows, "\n"))
	}
	first := paint(96)
	for _, key := range []string{"\x1b[1;2A", "\x1b[1;2B", "\x1b[1;3A"} {
		if layout.Key(m.View("main"), key, true).Action != terminalview.PromptPass {
			t.Fatal("preview collided with existing composer shortcut", key)
		}
	}
	if !strings.Contains(first, "preview line 01") || strings.Contains(first, "preview line 40") || !strings.Contains(first, "alt+shift+↑/↓") {
		t.Fatal(first)
	}
	for range 80 {
		key := layout.Key(m.View("main"), "\x1b[1;4B", true)
		if key.Action != terminalview.PromptHandled || key.Question.Kind != prompt.InputNone {
			t.Fatal(key)
		}
	}
	last := paint(96)
	if !strings.Contains(last, "preview line 40") || strings.Contains(last, "preview line 01") || m.Current().Selected != 0 {
		t.Fatal(last)
	}
	layout.Key(m.View("main"), "\x1b[1;4A", true)
	if paint(96) == last {
		t.Fatal("up did not scroll preview")
	}
	m.Current().Selected = 1
	narrow := paint(52)
	if strings.Contains(narrow, "│") || strings.Index(narrow, "Preview") < strings.Index(narrow, "Staged") || !strings.Contains(narrow, "Release plan") {
		t.Fatal(narrow)
	}
	for _, selected := range []int{2, 3} {
		m.Current().Selected = selected
		if got := paint(96); strings.Contains(got, "Preview") || strings.Contains(got, "Release plan") || strings.Contains(got, "alt+shift+↑/↓") {
			t.Fatal(got)
		}
	}
	m.Current().Selected = 0
	if !strings.Contains(paint(96), "preview line 01") {
		t.Fatal("focus change retained stale scroll")
	}
}

func TestQuestionPreviewPlainTextAndAsyncNote(t *testing.T) {
	m := previewDock()
	m.Current().Selected = 0
	m.Current().Choices[0].Preview = "<b>literal</b>\x1b[2J\x1b]52;c;hidden\a"
	var layout terminalview.QuestionLayout
	for _, width := range []int{48, 96} {
		rows := previewRows(m, &layout, width, 10)
		text := ansi.Strip(strings.Join(rows, "\n"))
		if !strings.Contains(text, "<b>literal</b>") || strings.Contains(text, "hidden") || !strings.Contains(text, "alt+shift+↑/↓") || len(rows) > 10 {
			t.Fatal(text)
		}
	}
	key := layout.Key(m.View("main"), "\t", true)
	if !m.ApplyInput(key.Question) || !m.Current().Note || m.Current().Done {
		t.Fatal("async Tab did not open an unsubmitted note")
	}
	m.Current().Complete("roll out slowly", false, false)
	if got := strings.Join(m.Current().Answer, "\n"); got != "Immediate\nroll out slowly" {
		t.Fatal(got)
	}
}

func TestQuestionRecommendedDescription(t *testing.T) {
	m := previewDock()
	m.Current().Choices[1].Preview = ""
	m.Current().Choices[1].Description = "Monitor before expanding"
	rows := previewRows(m, new(terminalview.QuestionLayout), 96, 14)
	text := ansi.Strip(strings.Join(rows, "\n"))
	if !strings.Contains(text, "❯ 2. Staged [Recommended]\n       Monitor before expanding") || m.Current().Selected != 1 || m.Current().Done {
		t.Fatal(text)
	}
}

func TestQuestionCompactHeightKeepsFocusedDescriptionReadable(t *testing.T) {
	c := prompt.Call{
		ID: "compact", Thread: "main",
		Questions: []prompt.Question{{
			ID:       "q1",
			Question: strings.Join([]string{"Line 1 of a long question", "Line 2 of a long question", "Line 3 of a long question", "Line 4 of a long question", "Line 5 of a long question"}, "\n"),
			Choices: []prompt.Option{
				{Label: "Narrow", Description: "Only one path"},
				{Label: "Broad", Description: "All paths"},
			},
		}},
	}
	var m prompt.Questions
	m.Calls = append(m.Calls, &c)
	m.Open(&c)
	var layout terminalview.QuestionLayout
	want := []string{"Only one path", "All paths", "Type your answer"}
	for idx, desc := range want {
		m.Current().Selected = idx
		rows := previewRows(&m, &layout, 80, 12)
		if len(rows) > 12 {
			t.Fatalf("option %d exceeded height: %d", idx, len(rows))
		}
		for _, row := range rows {
			if ansi.StringWidth(row) > 80 {
				t.Fatalf("option %d exceeded width: %q", idx, row)
			}
		}
		if text := ansi.Strip(strings.Join(rows, "\n")); !strings.Contains(text, desc) {
			t.Fatalf("option %d missing focused description %q in:\n%s", idx, desc, text)
		}
	}
}

func TestUISnapshotQuestionPreviews(t *testing.T) {
	for _, fixture := range []struct {
		name    string
		width   int
		preview bool
	}{
		{"question-preview-wide", 96, true}, {"question-preview-narrow", 52, true}, {"question-recommended-non-first", 80, false},
	} {
		t.Run(fixture.name, func(t *testing.T) {
			m := previewDock()
			if m.Current().Selected != 1 || m.Current().Done {
				t.Fatal("non-first recommendation did not focus without submitting")
			}
			if !fixture.preview {
				for i := range m.Current().Choices {
					m.Current().Choices[i].Preview = ""
				}
			}
			rows := previewRows(m, new(terminalview.QuestionLayout), fixture.width, 18)
			uisnapshot.Assert(t, "testdata/snapshots/"+fixture.name+".txt", strings.Join(rows, "\n")+"\n")
			uisnapshot.AssertTerminal(t, "testdata/snapshots/"+fixture.name+"-ansi.txt", append(rows, "plain after dock"), fixture.width)
			if m.Current().Choices[1].Label != "Staged" || m.Current().Done {
				t.Fatal("recommendation changed label or submitted")
			}
		})
	}
}

func TestQuestionMarkdownInQuestionNotesAndPreview(t *testing.T) {
	c := prompt.Call{
		ID: "md", Thread: "main",
		Questions: []prompt.Question{{
			ID:       "q1",
			Header:   "Choose `mode`",
			Question: "Update `Prompt.svelte` with `Markdown`?",
			Selected: 0,
			Choices: []prompt.Option{
				{
					Label:       "Use `Markdown` component",
					Description: "Render `code` spans and **bold** notes",
					Preview:     "Preview `inline`:\n\n```go\nfunc ok() bool { return true }\n```",
				},
				{
					Label:       "Include **/*.go",
					Description: "Leave `raw` text as-is",
				},
			},
		}},
	}
	var m prompt.Questions
	m.Calls = append(m.Calls, &c)
	m.Open(&c)
	for _, width := range []int{56, 96} {
		var layout terminalview.QuestionLayout
		painter := &terminalui.Painter{CopySource: true}
		rows := terminalview.Questions(m.View("main"), painter, &layout, "", false, width, 16)
		if !painter.CopySource {
			t.Fatalf("width=%d did not restore painter.CopySource", width)
		}
		for _, row := range rows {
			if ansi.StringWidth(row) > width {
				t.Fatalf("width=%d exceeded: %q", width, row)
			}
			if _, spans := terminalui.ExtractCopy(row); len(spans) > 0 {
				t.Fatalf("width=%d leaked copy markers into docked question: %q", width, row)
			}
		}
		plain := ansi.Strip(strings.Join(rows, "\n"))
		for _, forbidden := range []string{"`Prompt.svelte`", "`Markdown`", "`code`", "**bold**", "```go", "```", "`inline`"} {
			if strings.Contains(plain, forbidden) {
				t.Fatalf("width=%d rendered raw markdown %q in:\n%s", width, forbidden, plain)
			}
		}
		for _, expected := range []string{"Prompt.svelte", "Markdown", "code", "bold", "func ok() bool"} {
			if !strings.Contains(plain, expected) {
				t.Fatalf("width=%d missing rendered text %q in:\n%s", width, expected, plain)
			}
		}
		if width >= 80 && !strings.Contains(plain, "Include **/*.go") {
			t.Fatalf("width=%d missing glob option in:\n%s", width, plain)
		}
	}
	m.Current().Selected = 1
	narrowGlob := ansi.Strip(strings.Join(terminalview.Questions(m.View("main"), &terminalui.Painter{}, new(terminalview.QuestionLayout), "", false, 56, 16), "\n"))
	if !strings.Contains(narrowGlob, "Include **/*.go") {
		t.Fatalf("narrow selected glob corrupted:\n%s", narrowGlob)
	}
}

func TestUISnapshotQuestionDockMarkdown(t *testing.T) {
	c := prompt.Call{
		ID: "md-snap", Thread: "main",
		Questions: []prompt.Question{{
			ID:       "q1",
			Header:   "Choose `mode`",
			Question: "Update `Prompt.svelte` with `Markdown`?",
			Selected: 0,
			Choices: []prompt.Option{
				{
					Label:       "Use `Markdown`",
					Recommended: true,
					Description: "Render `code` and **bold** notes",
					Preview:     "Preview `inline`:\n\n```go\nfunc ok() bool { return true }\n```",
				},
				{
					Label:       "Keep `<pre>`",
					Description: "Leave `raw` text as-is",
				},
			},
		}},
	}
	var m prompt.Questions
	m.Calls = append(m.Calls, &c)
	m.Open(&c)
	for _, tc := range []struct {
		name  string
		theme diffrender.Theme
	}{
		{"dark", diffrender.DarkTheme},
		{"light", diffrender.LightTheme},
	} {
		t.Run(tc.name, func(t *testing.T) {
			rows := terminalview.Questions(m.View("main"), &terminalui.Painter{Theme: tc.theme}, new(terminalview.QuestionLayout), "", false, 96, 16)
			uisnapshot.AssertTerminal(t, "testdata/snapshots/question-markdown-"+tc.name+"-ansi.txt", append(rows, "plain after dock"), 96)
		})
	}
}
