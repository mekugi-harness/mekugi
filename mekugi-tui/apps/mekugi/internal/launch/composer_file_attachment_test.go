package launch

import (
	conversationmodel "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"

	"strings"
	"testing"
)

func TestComposerFileAttachmentLeavesOrdinaryUnicodeLimitToCodex(t *testing.T) {
	u, w := newAppServerTestUI()
	d := conversationmodel.Input{Text: strings.Repeat("界", 400_000)}
	if err := sendConversationInput(&u.attached, &u.Client, &u.picker, &u.questions, &u.approvals, u.clock, u.settings.Model, u.skillEnvironment, u.ownedImages, []conversationmodel.Input{d}, false); err != nil {
		t.Fatal(err)
	}
	requests := appServerTurnRequests(t, w)
	if len(requests) != 1 || requests[0].text() != d.Text {
		t.Fatal("attachment safety limit rejected an ordinary Unicode prompt")
	}
}
