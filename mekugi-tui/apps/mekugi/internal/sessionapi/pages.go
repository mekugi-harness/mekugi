package sessionapi

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

// Page budgets leave room for the RPC envelope and independent live state.

// Compact the presentation, not its raw transcript text. Disclosures retain
// their counts while their payload travels on the independent content channel.
// Anything removed from the inline view is acquired automatically by the reader.

// Match the JSON disclosure's expansion of embedded JSON strings without
// shipping or formatting their payload just to retain the disclosure count.

type PageCursor struct {
	Attachment string `json:"attachment"`
	Kind       string `json:"kind"`
	Revision   uint64 `json:"revision"`
	Before     uint64 `json:"before,omitzero"`
	Caller     string `json:"caller,omitzero"`
}

type ContentReference struct {
	Key        string `json:"key,omitzero"`
	Caller     string `json:"caller,omitzero"`
	Thread     string `json:"thread,omitzero"`
	Attachment string `json:"attachment"`
	Kind       string `json:"kind"`
	Revision   uint64 `json:"revision"`
	Seq        uint64 `json:"seq,omitzero"`
}

type PageResult struct {
	Live     []preview.Preview `json:"live,omitzero"`
	Roster   *roster           `json:"roster,omitzero"`
	Delivery string            `json:"delivery,omitzero"`
	Files    []reviewFile      `json:"files,omitzero"`
	Next     *PageCursor       `json:"next,omitzero"`
	Entries  []entry           `json:"entries"`
	Earlier  *PageCursor       `json:"earlier"`
}

type ContentResult struct {
	Session    *Session          `json:"session,omitzero"`
	Input      *Input            `json:"input,omitzero"`
	Prompts    *Prompts          `json:"prompts,omitzero"`
	Status     *status           `json:"status,omitzero"`
	ReviewInfo *savedReview      `json:"reviewInfo,omitzero"`
	Live       []preview.Preview `json:"live,omitzero"`
	Agent      *agent            `json:"agent,omitzero"`
	Review     *reviewFile       `json:"review,omitzero"`
	Entry      *entry            `json:"entry,omitzero"`
	Details    *NativeDetails    `json:"details,omitzero"`
}

// feedPage borrows the retained projection. Only the requested window is
// copied; a large entry keeps its complete public projection behind a reference.
