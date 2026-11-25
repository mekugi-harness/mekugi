// Source: apps/mekugi/internal/router/composer_selection.go:72:124@[e9da429a] quoted selection attachments.
package composer

import (
	"fmt"

	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/sourceanalysis/sourcekind"
)

const SelectionPrefix = "[Selected "
const UserSelectionsPrefix = "\n\n---\nUser selections:\n"

// UserSelections appends supplied text as ordinary Markdown in the user's input.
func (d Draft) UserSelections() string {
	var text strings.Builder
	appendSelection := func(label, source, body string) {
		if text.Len() == 0 {
			text.WriteString(UserSelectionsPrefix)
		}
		fmt.Fprintf(&text, "- %s:", label)
		format, _ := sourcekind.Classify(source)
		if !strings.ContainsAny(body, "\r\n") && format.Language == "" && !strings.HasPrefix(strings.ToLower(label), "diff") {
			text.WriteString(" " + body + "\n")
			return
		}
		longest, run := 0, 0
		for _, character := range body {
			if character == '`' {
				run++
				longest = max(longest, run)
			} else {
				run = 0
			}
		}
		fence := strings.Repeat("`", max(3, longest+1))
		text.WriteString("\n  " + fence + format.Language + "\n  ")
		text.WriteString(strings.ReplaceAll(body, "\n", "\n  "))
		if !strings.HasSuffix(body, "\n") {
			text.WriteString("\n  ")
		}
		text.WriteString(fence + "\n")
	}
	for _, selection := range d.Selections {
		label := strings.TrimSuffix(strings.TrimPrefix(d.Text[selection.Start:selection.End], SelectionPrefix), "]")
		appendSelection(label, selection.Source, selection.Text)
	}
	for _, file := range d.Files {
		if file.Source == "upload" {
			appendSelection("Uploaded "+file.Name, "", file.Text)
		}
	}
	return strings.TrimSuffix(text.String(), "\n")
}
