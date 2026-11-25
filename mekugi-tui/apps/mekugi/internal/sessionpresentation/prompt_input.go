package sessionpresentation

import "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"

type QuestionInputIntent struct {
	Version Version
	prompt.QuestionInput
}

type ApprovalInputIntent struct {
	Version Version
	prompt.ApprovalInput
}
