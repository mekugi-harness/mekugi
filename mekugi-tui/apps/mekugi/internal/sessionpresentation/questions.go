package sessionpresentation

import "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"

type AnswerIntent struct {
	Version Version
	prompt.AnswerInput
}

type QuestionSubmitIntent struct {
	Version    Version
	Call       string
	Incomplete bool
	Questions  []string
}
