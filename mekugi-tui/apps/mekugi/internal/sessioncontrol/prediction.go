package sessioncontrol

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"slices"
	"strconv"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
)

// PredictionSampleSupported follows the desktop's threadSample capability floor.
// Source: ChatGPT desktop 26.1120.50957, app-server-host.js threadSample.
func PredictionSampleSupported(version string) bool {
	return versionAtLeast(version, 0, 164, 0, 11)
}

// PredictionInterruptOnCancelSupported follows the desktop's
// threadSampleInterruptOnCancel capability floor.
// Source: ChatGPT desktop 26.1120.50957, app-server-host.js.
func PredictionInterruptOnCancelSupported(version string) bool {
	return versionAtLeast(version, 0, 167, 0, 1)
}

func versionAtLeast(version string, minMajor, minMinor, minPatch, minAlpha int) bool {
	version, _, _ = strings.Cut(version, "+")
	core, prerelease, _ := strings.Cut(version, "-")
	parts := strings.Split(core, ".")
	if len(parts) != 3 {
		return false
	}
	var numbers [3]int
	for i, part := range parts {
		value, err := strconv.Atoi(part)
		if err != nil || value < 0 {
			return false
		}
		numbers[i] = value
	}
	if numbers[0] != minMajor {
		return numbers[0] > minMajor
	}
	if numbers[1] != minMinor {
		return numbers[1] > minMinor
	}
	if numbers[2] != minPatch {
		return numbers[2] > minPatch
	}
	if prerelease == "" {
		return true
	}
	name, number, ok := strings.Cut(prerelease, ".")
	if !ok {
		return name == "beta" || name == "rc"
	}
	if name == "beta" || name == "rc" {
		return true
	}
	if name != "alpha" {
		return false
	}
	number, _, _ = strings.Cut(number, ".")
	alpha, err := strconv.Atoi(number)
	return err == nil && alpha >= minAlpha
}

// Prediction retains host correlation after its composer destination is retired.
type Prediction struct {
	SourceThread, SourceTurn, Trigger string
	Prompt, Effort                    string
	UnsupportedModels                 []string
	InterruptOnCancel                 bool
	Thread, Turn, Text                string
	Closed, Completed                 bool
	pending                           bool
	sampleRequestID                   string
	cancelSent                        bool
}

func (r *Requests[T]) sendPrediction(p *Prediction, method string, params any) (string, error) {
	var owner T
	id, err := r.Send(method, params, "", owner)
	if err == nil {
		request := r.pending[id]
		request.Prediction = p
		r.pending[id] = request
	} else {
		p.pending = false
	}
	return id, err
}

func (r *Requests[T]) StartPrediction(p *Prediction) error {
	p.pending = true
	_, err := r.sendPrediction(p, "thread/resume", map[string]any{"threadId": p.SourceThread})
	return err
}

// PredictionPending bounds auxiliary work while a prediction sample is still in flight.
func (r *Requests[T]) PredictionPending() bool {
	for _, request := range r.pending {
		if request.Consumer == r.consumer && request.Prediction != nil {
			return true
		}
	}
	return false
}

func (r *Requests[T]) HasPredictionThread(thread string) bool {
	return false
}

func (r *Requests[T]) ClosePrediction(p *Prediction) error {
	if p == nil {
		return nil
	}
	p.Closed = true
	if p.sampleRequestID == "" || p.cancelSent {
		return nil
	}
	p.cancelSent = true
	_, err := r.sendPrediction(p, "thread/sample/cancel", map[string]any{"requestId": rawRequestID(p.sampleRequestID)})
	if err != nil {
		p.cancelSent = false
	}
	return err
}

func rawRequestID(id string) any {
	val := jsontext.Value(id)
	if val.IsValid() {
		return val
	}
	return id
}

// ObservePrediction runs before Main/side admission and uses their one ledger.
// Source: ChatGPT desktop 26.1120.50957, early-bootstrap-916 sampleThread.
func (c *Client[T]) ObservePrediction(m appserver.Message, response Request[T]) (bool, error) {
	r := c.Requests
	p := response.Prediction
	if m.Method != "" || p == nil {
		return false, nil
	}
	if response.Method == "thread/sample/cancel" {
		return true, nil
	}
	p.pending = false
	if response.Method == "thread/sample" {
		p.sampleRequestID = ""
	}
	if m.Error != nil {
		return true, r.ClosePrediction(p)
	}
	switch response.Method {
	case "thread/resume":
		var result struct {
			Thread            binding.ThreadInfo `json:"thread"`
			Model             string             `json:"model"`
			ModelProvider     string             `json:"modelProvider"`
			CollaborationMode struct {
				Mode string `json:"mode"`
			} `json:"collaborationMode"`
		}
		if json.Unmarshal(m.Result, &result) != nil {
			return true, r.ClosePrediction(p)
		}
		if p.Closed || result.Thread.ID != p.SourceThread || result.ModelProvider != "openai" || result.CollaborationMode.Mode == "plan" || slices.Contains(p.UnsupportedModels, result.Model) {
			return true, r.ClosePrediction(p)
		}
		if turns := result.Thread.Turns; len(turns) == 0 || turns[len(turns)-1].ID != p.SourceTurn || turns[len(turns)-1].Status != "completed" {
			return true, r.ClosePrediction(p)
		}
		p.pending = true
		params := map[string]any{
			"threadId":       p.SourceThread,
			"expectedTurnId": p.SourceTurn,
			"prompt":         p.Prompt,
			"threadSource":   "composer_predictions",
			"turnTrigger":    "composer_predictions",
			"responsesapiClientMetadata": map[string]string{
				"prediction_source_thread_id": p.SourceThread,
				"prediction_source_turn_id":   p.SourceTurn,
				"prediction_trigger":          p.Trigger,
			},
			"disableMcp": true,
		}
		if p.Effort != "" {
			params["effort"] = p.Effort
		}
		if p.InterruptOnCancel {
			params["interruptOnCancel"] = true
		}
		id, err := r.sendPrediction(p, "thread/sample", params)
		if err == nil {
			p.sampleRequestID = id
		}
		return true, err
	case "thread/sample":
		if p.Closed {
			return true, nil
		}
		var result struct {
			Text *string `json:"text"`
		}
		if json.Unmarshal(m.Result, &result) != nil {
			return true, r.ClosePrediction(p)
		}
		p.Completed = true
		p.Text = ""
		if result.Text != nil && len(*result.Text) <= 64<<10 {
			var answer struct {
				Suggestion      *string  `json:"suggestion"`
				ConfidenceScore *float64 `json:"confidence_score"`
				PIqv3Low        *float64 `json:"p_iqv3_low"`
			}
			if json.Unmarshal([]byte(*result.Text), &answer, json.RejectUnknownMembers(true)) == nil && answer.Suggestion != nil {
				p.Text = strings.TrimSpace(*answer.Suggestion)
			}
		}
	}
	return true, nil
}
