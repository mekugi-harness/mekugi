package launch

import (
	"cmp"
	"strings"
)

type directPrediction struct {
	turn, text string
	revision   uint64
	attempted  bool
	pending    bool
}

// A reset invalidates the current attempt. Keep its slot occupied until the
// RPC settles so rapid turn or preference changes cannot stack samples.
func (c *directNativeConnection) resetPredictionLocked(turn string) {
	p := &c.prediction
	p.turn, p.text, p.attempted = turn, "", false
}

func (c *directNativeConnection) predictionAllowedLocked(thread, turn string) bool {
	return !c.closed && !c.predictionsDisabled && !c.archived && !c.busy &&
		c.opts.profile == "" &&
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
	if c.prediction.pending || c.prediction.attempted || !c.predictionAllowedLocked(thread, turn) {
		return
	}
	c.prediction.pending, c.prediction.attempted = true, true
	c.publishLocked()
	go c.samplePrediction(thread, turn, revision)
}

func (c *directNativeConnection) samplePrediction(thread, turn string, revision uint64) {
	var reply struct {
		Text string `json:"text"`
	}
	err := c.wire.Call(c.ctx, "thread/prediction/request", map[string]any{
		"threadId": thread, "sourceTurnId": turn, "automatic": true,
	}, &reply)
	c.mu.Lock()
	defer c.mu.Unlock()
	c.prediction.pending = false
	if err == nil && c.prediction.attempted && c.predictionAllowedLocked(thread, turn) {
		c.prediction.text = strings.TrimSpace(reply.Text)
		c.prediction.revision = revision
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
