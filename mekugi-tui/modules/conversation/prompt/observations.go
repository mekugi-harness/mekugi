package prompt

import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"

type Reply struct {
	ID          string
	Choice      int
	Note, Other string
	Skipped     bool
}

// Message contains decoded question facts, not host request or secret handles.
// HasReplies preserves reply-envelope identity even when no reply target decodes.
type Message struct {
	Ref                 observation.ItemRef
	Kind                observation.Kind
	Main, User, History bool
	Call                *Call
	ClientID            string
	HasReplies          bool
	Replies             []Reply
}
