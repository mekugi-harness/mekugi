package sessioncontrol

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
)

func TestPredictionCancelFailureCanRetry(t *testing.T) {
	transport := &requestTransport{id: "cancel"}
	r := NewRequests[struct{}](transport)
	client := &Client[struct{}]{Requests: r}
	p := &Prediction{sampleRequestID: "42"}
	transport.err = errors.New("write failed")
	if err := r.ClosePrediction(p); err == nil {
		t.Fatal("expected send error")
	}
	transport.err = nil
	if err := r.ClosePrediction(p); err != nil || len(transport.sends) != 2 || transport.sends[1].method != "thread/sample/cancel" {
		t.Fatalf("cancel retry failed: err=%v sends=%+v", err, transport.sends)
	}
	params := transport.sends[1].params.(map[string]any)
	if string(params["requestId"].(jsontext.Value)) != "42" {
		t.Fatalf("unexpected cancel requestId: %#v", params["requestId"])
	}
	request, _ := r.Take("cancel")
	if handled, err := client.ObservePrediction(appserver.Message{}, request); !handled || err != nil || r.PredictionPending() {
		t.Fatalf("cancel acknowledgement failed: handled=%v err=%v pending=%v", handled, err, r.PredictionPending())
	}
}

func TestPredictionHostCapabilityAndEligibility(t *testing.T) {
	for version, want := range map[string]bool{"": false, "0.163.0": false, "0.164.0-alpha.10": false, "0.164.0-alpha.11": true, "0.164.0": true, "0.172.0": true} {
		if PredictionSampleSupported(version) != want {
			t.Fatalf("capability %q", version)
		}
	}
	for version, want := range map[string]bool{"": false, "0.166.0": false, "0.167.0-alpha.0": false, "0.167.0-alpha.1": true, "0.167.0": true, "0.172.0": true} {
		if PredictionInterruptOnCancelSupported(version) != want {
			t.Fatalf("interruptOnCancel capability %q", version)
		}
	}
	for _, gate := range []string{"provider", "unsupported-model", "plan", "source-turn"} {
		t.Run(gate, func(t *testing.T) {
			transport := &requestTransport{id: "request"}
			requests := NewRequests[struct{}](transport)
			client := &Client[struct{}]{Requests: requests}
			p := &Prediction{SourceThread: "main", SourceTurn: "source", UnsupportedModels: []string{"blocked"}}
			result := map[string]any{"thread": map[string]any{"id": "main", "turns": []map[string]string{{"id": "source", "status": "completed"}}}, "modelProvider": "openai", "model": "gpt-6-astra"}
			switch gate {
			case "provider":
				result["modelProvider"] = "other"
			case "unsupported-model":
				result["model"] = "blocked"
			case "plan":
				result["collaborationMode"] = map[string]string{"mode": "plan"}
			case "source-turn":
				p.SourceTurn = "stale"
			}
			if err := requests.StartPrediction(p); err != nil {
				t.Fatal(err)
			}
			request, _ := requests.Take("request")
			body, _ := json.Marshal(result)
			if handled, err := client.ObservePrediction(appserver.Message{Result: body}, request); !handled || err != nil {
				t.Fatal(err)
			}
			if !p.Closed || len(transport.sends) != 1 {
				t.Fatal("ineligible source sampled")
			}
		})
	}
}

func TestPredictionSampleLifecycle(t *testing.T) {
	for _, closeAt := range []string{"", "resume", "sample"} {
		t.Run(closeAt, func(t *testing.T) {
			transport := &requestTransport{id: "request"}
			requests := NewRequests[struct{}](transport)
			client := &Client[struct{}]{Requests: requests}
			p := &Prediction{SourceThread: "main", SourceTurn: "source", Prompt: "remote", Trigger: "live", Effort: "low", InterruptOnCancel: true}
			if err := requests.StartPrediction(p); err != nil {
				t.Fatal(err)
			}
			reply := func(body string) {
				t.Helper()
				request, ok := requests.Take("request")
				if !ok {
					t.Fatal("missing ledger entry")
				}
				handled, err := client.ObservePrediction(appserver.Message{Result: jsontext.Value(body)}, request)
				if !handled || err != nil {
					t.Fatalf("reply handled=%v err=%v", handled, err)
				}
			}
			last := func() string { return transport.sends[len(transport.sends)-1].method }
			if closeAt == "resume" {
				_ = requests.ClosePrediction(p)
			}
			reply(`{"thread":{"id":"main","turns":[{"id":"source","status":"completed"}]},"model":"gpt-6-astra","modelProvider":"openai"}`)
			if closeAt == "resume" {
				if last() != "thread/resume" {
					t.Fatal("closed request sampled")
				}
				return
			}
			if last() != "thread/sample" {
				t.Fatal("did not sample")
			}
			params := transport.sends[len(transport.sends)-1].params.(map[string]any)
			if params["threadId"] != "main" || params["expectedTurnId"] != "source" || params["prompt"] != "remote" || params["threadSource"] != "composer_predictions" || params["turnTrigger"] != "composer_predictions" || params["disableMcp"] != true || params["interruptOnCancel"] != true || params["effort"] != "low" {
				t.Fatalf("unexpected thread/sample params: %+v", params)
			}
			if closeAt == "sample" {
				if err := requests.ClosePrediction(p); err != nil {
					t.Fatal(err)
				}
				if last() != "thread/sample/cancel" {
					t.Fatal("closed sample did not cancel")
				}
				reply(`{}`)
				return
			}
			reply(`{"sourceTurnId":"source","responseId":"resp-1","text":"{\"suggestion\":\"Show tests\",\"confidence_score\":0.9}"}`)
			if p.Text != "Show tests" || !p.Completed || p.Closed {
				t.Fatalf("lost sample result: %+v", p)
			}
			if requests.PredictionPending() || requests.Pending() {
				t.Fatal("prediction resources retained")
			}
		})
	}
}
