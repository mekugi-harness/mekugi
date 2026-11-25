package sessionapi

import "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"

const UploadLimit = 32 << 20

const InputBodyLimit = (UploadLimit+2)/3*4 + inputLimit

type Attachment struct {
	Source string  `json:"source"`
	Path   string  `json:"path,omitempty"`
	Name   string  `json:"name,omitempty"`
	Data   *string `json:"data,omitzero"`
	Inline bool    `json:"inline,omitzero"`
}

type InputRequest struct {
	Text        string       `json:"text"`
	Attachments []Attachment `json:"attachments,omitempty"`
	// NativeDraft is supplied by the private native transport after decoding its
	// semantic composer request. Ordinary JSON input cannot set it.
	NativeDraft *composer.Draft `json:"-"`
}

// InputError is a permanent preparation error, not a retryable admission refusal.
type InputError struct {
	Message string
	Limit   bool
}

func (e *InputError) Error() string { return e.Message }
