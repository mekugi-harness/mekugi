package binding

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
)

type QuestionAnswer struct {
	ID      string `json:"id"`
	Choice  int    `json:"choice,omitzero"`
	Note    string `json:"note,omitzero"`
	Other   string `json:"other,omitzero"`
	Skipped bool   `json:"skipped,omitzero"`
}

func EncodeQuestionResponse(answers []QuestionAnswer) map[string]any {
	if answers == nil {
		answers = []QuestionAnswer{}
	}
	return map[string]any{"answers": answers}
}

type Question = observation.Question
type QuestionRequest struct {
	ThreadID  string     `json:"threadId"`
	TurnID    string     `json:"turnId"`
	ItemID    string     `json:"itemId"`
	Questions []Question `json:"questions"`
}

func DecodeQuestionRequest(params jsontext.Value) (QuestionRequest, error) {
	var wire struct {
		ThreadID  string           `json:"threadId"`
		TurnID    string           `json:"turnId"`
		ItemID    string           `json:"itemId"`
		Questions []jsontext.Value `json:"questions"`
	}
	if err := json.Unmarshal(params, &wire); err != nil {
		return QuestionRequest{}, err
	}
	p := QuestionRequest{
		ThreadID: wire.ThreadID,
		TurnID:   wire.TurnID,
		ItemID:   wire.ItemID,
	}
	if wire.Questions != nil {
		p.Questions = make([]Question, len(wire.Questions))
		for i, raw := range wire.Questions {
			if err := p.Questions[i].UnmarshalJSON(raw); err != nil {
				return QuestionRequest{}, err
			}
		}
	}
	return p, nil
}

type ResolvedRequest struct {
	ThreadID  string         `json:"threadId"`
	RequestID jsontext.Value `json:"requestId"`
}

func DecodeResolvedRequest(params jsontext.Value) (ResolvedRequest, error) {
	var p ResolvedRequest
	err := json.Unmarshal(params, &p)
	return p, err
}
