package terminalview

import (
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/textsanitize"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
)

// ErrorHasDetails reports content omitted by the inline first-line preview.
func ErrorHasDetails(block Block) bool {
	return strings.TrimSpace(textsanitize.Safe(block.Body, false)) != presentation.ErrorPreview(block.Body)
}

// ErrorRows adds a dialog hint only when the preview omits retained content.
func ErrorRows(theme diffrender.Theme, block Block, width int) []string {
	rows := terminalui.Wrap(theme.Danger()+presentation.ErrorPreview(block.Body)+terminalui.Reset, width, false)
	if !ErrorHasDetails(block) {
		return rows
	}
	hint := "details · ctrl+b !"
	if block.Hovered {
		hint = Underline(hint)
	}
	return append(rows, terminalui.Wrap(terminalui.Dim+hint+terminalui.Undim, width, false)...)
}
