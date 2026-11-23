package sessionpresentation

import (
	"encoding/json/jsontext"

	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	conversationbinding "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/codexbinding"
)

// UserText maps Conversation's decoded token identities into display annotations.
func UserText(content jsontext.Value) (string, []activitypresentation.TextSpan, bool) {
	text, spans, ok := conversationbinding.UserText(content)
	return text, TokenSpans(spans), ok
}
