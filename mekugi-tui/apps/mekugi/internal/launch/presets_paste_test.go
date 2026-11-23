package launch

import (
	"testing"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/terminalsession"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
)

func TestPresetNativePasteKeepsDraft(t *testing.T) {
	u, _ := newAppServerTestUI()
	u.startSession("main", "/workspace")
	c := nativeTestPromptClient(u)
	c.editor("main").InsertText("ordinary draft")
	c.event.Frame.View.Menu = &sessionpresentation.MenuView{ID: 1, Kind: "presets", Title: "Presets", Choices: []completion.Choice{{Name: "1", Display: "1 empty"}}}
	c.refresh(time.Now())
	for _, event := range []terminalsession.Input{{Kind: terminalsession.PasteStart}, {Kind: terminalsession.PasteInput, Text: "UNINTENDED"}, {Kind: terminalsession.PasteEnd}} {
		c.dispatch(event)
	}
	t.Logf("draft after quick-menu paste=%q", c.editor("main").Text)
	if c.editor("main").Text != "ordinary draft" {
		t.Error("Pasting in the quick menu mutated the ordinary draft")
	}
}
