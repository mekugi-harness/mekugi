package launch

import (
	"cmp"
	json "encoding/json/v2"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
)

type directPrediction struct {
	turn, text string
	revision   uint64
	attempted  bool
	pending    *directPredictionRequest
}

type directPredictionRequest struct {
	thread, turn string
	revision     uint64
	discard      bool
}

// resetPredictionLocked also retires a notification that may still be in flight.
// Keep its request occupied until it settles: notifications identify only the
// source, so two requests for one turn cannot be distinguished.
func (c *directNativeConnection) resetPredictionLocked(turn string) {
	p := &c.prediction
	p.turn, p.text, p.attempted = turn, "", false
	if p.pending != nil {
		p.pending.discard = true
	}
}

func (c *directNativeConnection) predictionAllowedLocked(thread, turn string) bool {
	return !c.closed && !c.predictionsDisabled && !c.archived && !c.busy &&
		c.opts.profile == "" && c.modelProvider == "openai" &&
		(c.predictionChatGPT == nil || *c.predictionChatGPT) &&
		thread != "" && thread == c.mainThread &&
		(c.family == nil || c.family.Root == thread) &&
		cmp.Or(c.inputThread, c.mainThread) == thread && cmp.Or(c.displayedThread, c.mainThread) == thread &&
		turn != "" && turn == c.prediction.turn && len(c.mainQueue.pending) == 0
}

// requestPrediction stages a native sample without blocking terminal input or
// serializing other commands behind the auxiliary model response.
func (c *directNativeConnection) requestPrediction(thread, turn string, revision uint64) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.prediction.pending != nil || c.prediction.attempted || !c.predictionAllowedLocked(thread, turn) {
		return
	}
	req := &directPredictionRequest{thread: thread, turn: turn, revision: revision}
	c.prediction.pending, c.prediction.attempted = req, true
	c.publishLocked()
	go c.samplePrediction(req)
}

func (c *directNativeConnection) samplePrediction(req *directPredictionRequest) {
	c.mu.Lock()
	if c.predictionChatGPT == nil {
		c.mu.Unlock()
		var account struct {
			Account *struct {
				Type string `json:"type"`
			} `json:"account"`
		}
		err := c.wire.Call(c.ctx, "account/read", map[string]bool{"refreshToken": false}, &account)
		c.mu.Lock()
		// Account notifications received during this read take precedence.
		if err == nil && c.predictionChatGPT == nil {
			c.predictionChatGPT = new(account.Account != nil && account.Account.Type == "chatgpt")
		}
	}
	send := c.predictionChatGPT != nil && !req.discard && c.predictionAllowedLocked(req.thread, req.turn)
	c.mu.Unlock()
	if send && c.wire.Call(c.ctx, "thread/prediction/request", map[string]string{
		"threadId":     req.thread,
		"sourceTurnId": req.turn,
	}, nil) == nil {
		// The native server publishes the result separately from its reply.
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.prediction.pending == req {
		c.prediction.pending = nil
		c.publishLocked()
	}
}

func (c *directNativeConnection) updatePredictionAccount(msg appserver.Message) {
	var account struct {
		AuthMode string `json:"authMode"`
	}
	if json.Unmarshal(msg.Params, &account) != nil {
		return
	}
	chatGPT := account.AuthMode == "chatgpt" || account.AuthMode == "chatgptAuthTokens"
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.predictionChatGPT == nil || *c.predictionChatGPT != chatGPT {
		c.predictionChatGPT = &chatGPT
		c.resetPredictionLocked(c.prediction.turn)
		c.publishLocked()
	}
}

func (c *directNativeConnection) handlePrediction(msg appserver.Message) {
	var p struct {
		ThreadID     string `json:"threadId"`
		SourceTurnID string `json:"sourceTurnId"`
		Result       struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"result"`
	}
	if json.Unmarshal(msg.Params, &p) != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	req := c.prediction.pending
	if req == nil || req.thread != p.ThreadID || req.turn != p.SourceTurnID {
		return
	}
	c.prediction.pending = nil
	if !req.discard && c.predictionAllowedLocked(req.thread, req.turn) && p.Result.Type == "completed" {
		c.prediction.text = strings.TrimSpace(p.Result.Text)
		c.prediction.revision = req.revision
	}
	c.publishLocked()
}

func (c *nativeClient) acquirePrediction() {
	frame := c.event.Frame
	view := c.renderer.frame.View
	turn := frame.Facts.PredictionTurn
	if turn == "" || !c.notifications.focused || c.reattach != "" || c.attaching != "" ||
		c.renderer.terminal.Main.PredictionsDisabled || !view.Draft.Available ||
		view.Main.ID != view.Input.ID || view.Main.ID != view.Displayed.ID ||
		view.Questions.Call != nil || view.Approvals.Open || len(frame.Questions)+len(frame.Approvals) != 0 ||
		frame.Facts.Busy || len(view.Pending.Queued) != 0 || !c.inputAvailable() {
		return
	}
	editor := c.editor("main")
	if editor.Text == "" && len(editor.Images) == 0 {
		c.connection.requestPrediction(view.Main.ID, turn, editor.Revision())
	}
}
