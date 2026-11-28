package terminalview

import (
	"strings"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

// Question is a presentation snapshot, never an editable or transport record.
type Question = presentation.Question

func (p *Painter) QuestionState(state string) string {
	color := terminalui.Dim
	switch state {
	case "answered", "answered elsewhere":
		color = "\x1b[38;2;165;214;167m"
		if p.Theme == diffrender.LightTheme {
			color = "\x1b[38;2;10;102;52m"
		}
	case "open", "waiting", "sending":
		color = p.Theme.Accent()
	}
	return color + state + terminalui.Reset
}

func (p *Painter) questionRows(block Block, width int) []string {
	count, state, _ := strings.Cut(block.Label, " · ")
	pad := 2
	if count != "" {
		pad = max(2, block.VerbColumn-len("Asked"))
	}
	header := p.Theme.Accent() + "Asked" + terminalui.Reset + strings.Repeat(" ", pad) + terminalui.Dim + count + terminalui.Undim
	if state != "" {
		header += " · " + p.QuestionState(state)
	}
	rows := terminalui.Wrap(header, width, false)
	inner := max(1, width-2)
	showPerQuestionState := false
	if len(block.Questions) > 1 {
		for _, q := range block.Questions {
			if q.State != "" && q.State != state {
				showPerQuestionState = true
				break
			}
		}
	}
	for i, q := range block.Questions {
		if i > 0 {
			rows = append(rows, "")
		}
		prevCopy := p.CopySource
		p.CopySource = false
		qLines := p.MarkdownComplete(q.Text, inner, false)
		p.CopySource = prevCopy
		if len(qLines) == 0 && q.Text != "" {
			qLines = []string{""}
		}
		if showPerQuestionState && q.State != "" {
			badge := p.QuestionState(q.State)
			if len(qLines) > 0 && ansi.StringWidth(qLines[len(qLines)-1])+3+ansi.StringWidth(badge) <= inner {
				qLines[len(qLines)-1] += terminalui.Dim + " · " + terminalui.Undim + badge
			} else {
				qLines = append(qLines, badge)
			}
		}
		rows = append(rows, liveActivityIndent(qLines, "  ")...)
		switch {
		case q.Answer != "":
			answer := p.Markdown(q.Answer, max(1, width-4), false)
			for j, row := range answer {
				prefix := "    "
				if j == 0 {
					prefix = "  " + p.Theme.Accent() + "└ " + terminalui.Reset
				}
				rows = append(rows, prefix+row)
			}
		case q.Options != "":
			p.CopySource = false
			inlineOptions := p.InlineComplete(q.Options)
			p.CopySource = prevCopy
			options := strings.NewReplacer(
				terminalui.Reset, terminalui.Reset+terminalui.Dim,
				terminalui.Undim, terminalui.Undim+terminalui.Dim,
			).Replace(inlineOptions)
			rows = append(rows, liveActivityIndent(terminalui.Wrap(terminalui.Dim+options+terminalui.Reset, inner, false), "  ")...)
		}
	}
	for i, row := range rows {
		rows[i] = ansi.Truncate(row, width, "")
	}
	return rows
}
