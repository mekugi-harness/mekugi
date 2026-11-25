// Package sessionapi defines wire protocol DTOs, schema generation, and
// peer-checked local Unix socket transport for the connector and TUI client.
package sessionapi

import "encoding/json/jsontext"

// Action is one renderer-neutral request from a viewer.
type Action string

const (
	TogglePause Action = "toggle-pause"
	Faster      Action = "faster"
	Slower      Action = "slower"
	SeekBack    Action = "back"
	SeekForward Action = "forward"
	Restart     Action = "restart"
	Seek        Action = "seek"

	Send            Action = "send"
	Stop            Action = "stop"
	AnswerQuestions Action = "answer"
	DecideApproval  Action = "decide"
	Redraw          Action = "redraw"
	RunShell        Action = "shell"
)

type Answer struct {
	Call    string           `json:"call"`
	Answers []QuestionAnswer `json:"answers"`
}

type QuestionAnswer struct {
	Note        string       `json:"note,omitzero"`
	Question    string       `json:"question"`
	Choice      *int         `json:"choice,omitzero"`
	Text        string       `json:"text,omitzero"`
	Skip        bool         `json:"skip,omitzero"`
	Attachments []Attachment `json:"attachments,omitempty"`
}

type Decide struct {
	Approval string `json:"approval"`
	Choice   int    `json:"choice"`
	Reason   string `json:"reason,omitzero"`
}

const inputLimit = 1 << 20

type feedPatch struct {
	Attachment string           `json:"attachment"`
	Entries    []jsontext.Value `json:"entries,omitzero"`
	Order      []uint64         `json:"order,omitzero"`
	Revision   uint64           `json:"revision"`
	Earlier    *PageCursor      `json:"earlier"`
}

type message struct {
	Deferred *DeferredSections `json:"deferred"`
	Session  *Session          `json:"session,omitzero"`
	Playback *Playback         `json:"playback,omitzero"`
	Input    *Input            `json:"input,omitzero"`
	Prompts  jsontext.Value    `json:"prompts,omitzero"`
	Status   jsontext.Value    `json:"status,omitzero"`
	Main     *feedPatch        `json:"main,omitzero"`
	Agents   *feedPatch        `json:"agents,omitzero"`
	Roster   jsontext.Value    `json:"roster,omitzero"`
	Review   jsontext.Value    `json:"review,omitzero"`
	Live     jsontext.Value    `json:"live,omitzero"`
}
