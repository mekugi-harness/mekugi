package launch

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"

	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
	previewbinding "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/codexbinding"
	previewview "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/terminalview"
)

const nativeCodeModeInputDelta = "item/codeMode/inputDelta"

type nativeInputPreview struct {
	turn, call string
	next       int
	stream     *previewbinding.Stream
}

func (p *nativeInputPreview) stop() {
	if p.stream == nil {
		return
	}
	p.stream.Stop()
	*p = nativeInputPreview{}
}

func (s *nativeSession) previewInput(attached *attachedPresentation, params []byte) error {
	var event struct {
		Thread string `json:"threadId"`
		Turn   string `json:"turnId"`
		Call   string `json:"callId"`
		Offset int    `json:"offset"`
		Delta  string `json:"delta"`
	}
	if err := json.Unmarshal(params, &event); err != nil {
		return err
	}
	if event.Thread != s.Client.Thread || event.Turn == "" || event.Turn != s.Client.Turn ||
		event.Call == "" || event.Delta == "" {
		return nil
	}
	p := &s.inputPreview
	if event.Offset == 0 {
		p.stop()
		broker := s.review.Preview()
		workspace := s.review.WorkspaceForThread(event.Thread)
		if !broker.Enabled() || workspace == "" {
			return nil
		}
		caller := attached.session.KnownCaller(event.Thread)
		p.turn, p.call = event.Turn, event.Call
		p.stream = previewbinding.Start(s.ctx, broker,
			previewbinding.Identity{Workspace: workspace, Thread: event.Thread, Turn: event.Turn, Caller: caller, Kind: "exec"},
			previewbinding.Timing{Frame: previewview.PreviewFrameDelay, Unit: previewview.PreviewUnitDelay, Finish: previewview.PreviewFinishDrain},
			changecapture.ReadPatchFile)
	}
	if p.stream == nil || p.turn != event.Turn || p.call != event.Call {
		return nil
	}
	if p.next != event.Offset {
		p.stop()
		return nil
	}
	p.next += len(event.Delta)
	p.stream.Append(event.Delta)
	return nil
}

func (p *nativeInputPreview) finish(turn string, raw []byte) error {
	if p.stream == nil || p.turn != turn {
		return nil
	}
	var item struct {
		Type  string         `json:"type"`
		Call  string         `json:"call_id"`
		Input jsontext.Value `json:"input"`
	}
	if err := json.Unmarshal(raw, &item); err != nil {
		return err
	}
	if item.Type == "custom_tool_call" && item.Call == p.call {
		var input string
		if err := json.Unmarshal(item.Input, &input); err != nil {
			return err
		}
		p.stream.Finish(input)
	}
	return nil
}
