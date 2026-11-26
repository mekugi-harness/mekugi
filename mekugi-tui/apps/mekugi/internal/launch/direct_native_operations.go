package launch

import (
	"context"
	"crypto/rand"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"net/http"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	activityfeed "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/codexbinding"
)

func (c *directNativeConnection) Send(ctx context.Context, _ sessionapi.NativeTarget, op sessionapi.NativeOperation) (sessionapi.NativeResult, error) {
	var err error
	switch {
	case op.Composer != nil:
		err = c.handleComposer(ctx, *op.Composer)
	case op.Command != nil:
		err = c.handleSlashCommand(ctx, *op.Command)
	case op.Interrupt != nil:
		err = c.handleInterrupt(ctx, *op.Interrupt)
	case op.Approval != nil:
		err = c.handleApproval(ctx, *op.Approval)
	case op.Answers != nil:
		err = c.handleAnswers(ctx, *op.Answers)
	case op.Display != nil:
		err = c.handleDisplay(ctx, *op.Display)
	case op.Side != nil:
		err = c.handleCloseSide(ctx)
	case op.BTW != nil:
		err = c.handleCloseBTW(ctx)
	case op.Resume != nil:
		err = c.handleResumeAction(ctx, *op.Resume)
	case op.Output != nil:
		c.handleOutputDemand(*op.Output)
	case op.DetailOutput != nil:
		c.handleDetailOutput(*op.DetailOutput)
	case op.CloseDetails != nil:
		c.mu.Lock()
		c.details = nil
		c.publishLocked()
		c.mu.Unlock()
	case op.Preferences != nil:
		c.mu.Lock()
		c.preferences = slices.Clone(op.Preferences.Presentation)
		c.publishLocked()
		c.mu.Unlock()
	case op.Focused != nil:
		c.mu.Lock()
		c.focused = *op.Focused
		c.mu.Unlock()
	case op.Completion != nil:
		c.handleCompletion(ctx, *op.Completion)
	case op.Menu != nil:
		err = c.handleMenu(ctx, *op.Menu, op.Editor)
	case op.Receipt != nil:
		c.handleReceipt(*op.Receipt)
	case op.SaveDraft != nil, op.Panel != nil, op.Details != nil, op.Notice != nil, op.Recovery != nil, op.Reference != nil, op.StopProcess != nil:
		// No-op or handled locally; return current version.
	}
	c.mu.Lock()
	ver := sessionpresentation.Version{Attachment: c.attachID, Revision: c.revision}
	c.mu.Unlock()
	return sessionapi.NativeResult{Version: ver}, err
}

func (c *directNativeConnection) handleComposer(ctx context.Context, in sessionapi.NativeComposerInput) error {
	text := strings.TrimSpace(in.Draft.Text)
	switch in.Intent.Operation {
	case sessionpresentation.IncreaseEffort:
		return c.stepReasoningEffort(ctx, +1)
	case sessionpresentation.DecreaseEffort:
		return c.stepReasoningEffort(ctx, -1)
	case sessionpresentation.SteerWaiting:
		c.mu.Lock()
		threadID := c.inputThread
		if threadID == "" {
			threadID = c.mainThread
		}
		c.mu.Unlock()
		if threadID == "" {
			return errors.New("no active thread")
		}
		return c.wire.Call(ctx, "thread/queue/start", map[string]any{"threadId": threadID}, nil)
	}
	hasAttachments := len(in.Draft.Images) > 0 || len(in.Draft.AnswerImages) > 0 || in.Draft.AnswerSelections != "" || in.Draft.UserSelections() != ""
	if text == "" && !hasAttachments {
		return nil
	}
	if strings.HasPrefix(text, "/") {
		if hasAttachments {
			return &sessionapi.NativeFailure{Status: http.StatusBadRequest, Reason: "attachments are not supported with slash commands"}
		}
		return c.handleSlashCommand(ctx, text)
	}
	if strings.HasPrefix(text, "!") {
		if hasAttachments {
			return &sessionapi.NativeFailure{Status: http.StatusBadRequest, Reason: "attachments are not supported with shell commands"}
		}
		cmd := strings.TrimSpace(strings.TrimPrefix(text, "!"))
		if cmd == "" {
			return nil
		}
		c.mu.Lock()
		threadID := c.mainThread
		c.mu.Unlock()
		if threadID == "" {
			return errors.New("no active thread")
		}
		return c.wire.Call(ctx, "thread/shellCommand", map[string]any{
			"threadId": threadID,
			"command":  cmd,
		}, nil)
	}

	c.mu.Lock()
	threadID := in.Intent.Thread
	if threadID == "" {
		threadID = c.inputThread
	}
	if threadID == "" {
		threadID = c.mainThread
	}
	turnID := c.activeTurnForLocked(threadID)
	busy := turnID != ""
	c.mu.Unlock()
	if threadID == "" {
		return errors.New("no active thread")
	}
	input := codexbinding.Input(in.Draft)
	if in.Intent.Operation == sessionpresentation.QueueComposer && busy {
		return c.wire.Call(ctx, "thread/queue/add", map[string]any{
			"threadId":            threadID,
			"input":               input,
			"clientUserMessageId": rand.Text(),
		}, nil)
	}
	if busy {
		params := map[string]any{
			"threadId": threadID,
			"input":    input,
		}
		if turnID != "" {
			params["expectedTurnId"] = turnID
		}
		return c.wire.Call(ctx, "turn/steer", params, nil)
	}
	return c.wire.Call(ctx, "turn/start", map[string]any{
		"threadId": threadID,
		"input":    input,
	}, nil)
}

func (c *directNativeConnection) handleSlashCommand(ctx context.Context, raw string) error {
	trimmed := strings.TrimSpace(raw)
	verb, rest, _ := strings.Cut(trimmed, " ")
	rest = strings.TrimSpace(rest)
	c.mu.Lock()
	mainID := c.mainThread
	c.mu.Unlock()

	switch verb {
	case "/quit", "/exit", "/detach":
		c.finish(&sessionapi.NativeExit{Code: 0})
		return nil
	case "/compact":
		if mainID == "" {
			return errors.New("no active thread")
		}
		return c.wire.Call(ctx, "thread/compact/start", map[string]any{"threadId": mainID}, nil)
	case "/archive":
		if mainID == "" {
			return errors.New("no active thread")
		}
		if err := c.wire.Call(ctx, "thread/archive", map[string]any{"threadId": mainID}, nil); err != nil {
			return err
		}
		c.mu.Lock()
		c.archived = true
		c.publishLocked()
		c.mu.Unlock()
		return nil
	case "/new", "/clear":
		return c.startFreshThread(ctx)
	case "/resume":
		if rest != "" {
			return c.resumeThread(ctx, rest)
		}
		return c.loadResumeList(ctx, "", false)
	case "/rewind":
		if mainID == "" {
			return errors.New("no active thread")
		}
		beforeTurnID := rest
		if beforeTurnID == "" {
			resolved, err := c.resolveLatestTurnID(ctx, mainID)
			if err != nil {
				return err
			}
			beforeTurnID = resolved
		}
		return c.wire.Call(ctx, "thread/revert", map[string]any{
			"threadId":     mainID,
			"beforeTurnId": beforeTurnID,
		}, nil)
	case "/model":
		if mainID == "" {
			return errors.New("no active thread")
		}
		if rest == "" {
			return c.openModelMenu(ctx)
		}
		return c.wire.Call(ctx, "thread/settings/update", map[string]any{
			"threadId": mainID,
			"model":    rest,
		}, nil)
	case "/effort", "/reasoning":
		if mainID == "" {
			return errors.New("no active thread")
		}
		if rest == "" {
			return c.openEffortMenu(ctx)
		}
		return c.wire.Call(ctx, "thread/settings/update", map[string]any{
			"threadId": mainID,
			"effort":   rest,
		}, nil)
	case "/tier":
		if mainID == "" {
			return errors.New("no active thread")
		}
		if rest == "" {
			return c.openTierMenu(ctx)
		}
		return c.applyServiceTier(ctx, mainID, rest)
	case "/fast":
		if mainID == "" {
			return errors.New("no active thread")
		}
		c.mu.Lock()
		nextTier := "fast"
		if c.tier == "fast" {
			nextTier = ""
		}
		c.mu.Unlock()
		return c.applyServiceTier(ctx, mainID, nextTier)
	case "/side":
		return c.startSide(ctx, rest)
	case "/btw", "/one-off":
		return c.startBTW(ctx, rest)
	default:
		c.mu.Lock()
		c.setFeedbackLocked("Unknown command: "+verb, true)
		c.publishLocked()
		c.mu.Unlock()
		return nil
	}
}

func (c *directNativeConnection) resolveLatestTurnID(ctx context.Context, threadID string) (string, error) {
	var page struct {
		Data []struct {
			ID string `json:"id"`
		} `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/turns/list", map[string]any{
		"threadId":      threadID,
		"sortDirection": "desc",
		"limit":         1,
		"itemsView":     "notLoaded",
	}, &page); err == nil && len(page.Data) > 0 && page.Data[0].ID != "" {
		return page.Data[0].ID, nil
	}
	var resp struct {
		Thread directWireThread `json:"thread"`
	}
	if err := c.wire.Call(ctx, "thread/read", map[string]any{"threadId": threadID, "includeTurns": true}, &resp); err == nil {
		for i := len(resp.Thread.Turns) - 1; i >= 0; i-- {
			if resp.Thread.Turns[i].ID != "" {
				return resp.Thread.Turns[i].ID, nil
			}
		}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if threadID == c.mainThread && c.activeTurn != "" {
		return c.activeTurn, nil
	}
	records := c.items[threadID]
	for i := len(records) - 1; i >= 0; i-- {
		if records[i].turnID != "" {
			return records[i].turnID, nil
		}
	}
	return "", errors.New("no turns available to rewind")
}

func (c *directNativeConnection) stepReasoningEffort(ctx context.Context, delta int) error {
	levels := []string{"low", "medium", "high", "xhigh"}
	c.mu.Lock()
	mainID := c.mainThread
	cur := c.effort
	c.mu.Unlock()
	if mainID == "" {
		return nil
	}
	idx := slices.Index(levels, cur)
	if idx < 0 {
		idx = 1
	}
	next := min(max(idx+delta, 0), len(levels)-1)
	return c.wire.Call(ctx, "thread/settings/update", map[string]any{
		"threadId": mainID,
		"effort":   levels[next],
	}, nil)
}

func (c *directNativeConnection) startSide(ctx context.Context, promptText string) error {
	c.mu.Lock()
	mainID := c.mainThread
	sideID := c.sideThread
	c.mu.Unlock()
	if mainID == "" {
		return errors.New("no active thread")
	}
	if sideID == "" {
		var resp directThreadStartResponse
		if err := c.wire.Call(ctx, "thread/fork", map[string]any{
			"threadId":  mainID,
			"ephemeral": true,
		}, &resp); err != nil {
			return fmt.Errorf("fork side thread: %w", err)
		}
		sideID = resp.Thread.ID
		c.mu.Lock()
		c.sideThread = sideID
		c.inputThread = sideID
		c.displayedThread = sideID
		c.threadCallers[sideID] = "side"
		c.publishLocked()
		c.mu.Unlock()
	} else {
		c.mu.Lock()
		c.inputThread = sideID
		c.displayedThread = sideID
		c.publishLocked()
		c.mu.Unlock()
	}
	if promptText != "" {
		return c.wire.Call(ctx, "turn/start", map[string]any{
			"threadId": sideID,
			"input":    appserver.Input(promptText),
		}, nil)
	}
	return nil
}

func (c *directNativeConnection) handleCloseSide(ctx context.Context) error {
	c.mu.Lock()
	sideID := c.sideThread
	c.sideThread = ""
	c.sideActiveTurn = ""
	c.inputThread = c.mainThread
	c.displayedThread = c.mainThread
	c.publishLocked()
	c.mu.Unlock()
	if sideID != "" {
		_ = c.wire.Call(ctx, "thread/archive", map[string]any{"threadId": sideID}, nil)
	}
	return nil
}

func (c *directNativeConnection) startBTW(ctx context.Context, question string) error {
	if question == "" {
		return errors.New("provide a question for /btw")
	}
	c.mu.Lock()
	mainID := c.mainThread
	btwID := c.btwThread
	btwMain := c.btwMainThread
	btwRunning := c.btwRunning
	c.mu.Unlock()
	if mainID == "" {
		return errors.New("no active thread")
	}
	if btwID != "" && btwMain != mainID {
		_ = c.wire.Call(ctx, "thread/archive", map[string]any{"threadId": btwID}, nil)
		btwID = ""
		btwRunning = false
	}
	if btwRunning {
		return &sessionapi.NativeFailure{Status: http.StatusConflict, Reason: "wait for the current /btw answer to finish"}
	}
	if btwID == "" {
		var resp directThreadStartResponse
		if err := c.wire.Call(ctx, "thread/fork", map[string]any{
			"threadId":       mainID,
			"threadSource":   "mekugi_btw",
			"excludeTurns":   true,
			"sandbox":        "read-only",
			"approvalPolicy": "never",
		}, &resp); err != nil {
			return fmt.Errorf("fork one-off thread: %w", err)
		}
		btwID = resp.Thread.ID
	}
	c.mu.Lock()
	c.btwThread = btwID
	c.btwMainThread = mainID
	c.btwExchangeID++
	c.btwQuestion = question
	c.btwAnswer.Reset()
	c.btwRunning = true
	c.publishLocked()
	c.mu.Unlock()
	return c.wire.Call(ctx, "turn/start", map[string]any{
		"threadId":     btwID,
		"input":        appserver.Input(question),
		"environments": []any{},
	}, nil)
}

func (c *directNativeConnection) handleCloseBTW(ctx context.Context) error {
	c.mu.Lock()
	btwID := c.btwThread
	c.btwThread = ""
	c.btwMainThread = ""
	c.btwQuestion = ""
	c.btwAnswer.Reset()
	c.btwRunning = false
	c.publishLocked()
	c.mu.Unlock()
	if btwID != "" {
		_ = c.wire.Call(ctx, "thread/archive", map[string]any{"threadId": btwID}, nil)
	}
	return nil
}

func (c *directNativeConnection) handleInterrupt(ctx context.Context, intent sessionpresentation.InterruptIntent) error {
	c.mu.Lock()
	threadID := intent.Target.Thread
	if threadID == "" {
		threadID = c.inputThread
	}
	if threadID == "" {
		threadID = c.mainThread
	}
	turnID := intent.Target.Turn
	if turnID == "" {
		turnID = c.activeTurnForLocked(threadID)
	}
	c.mu.Unlock()
	if threadID == "" {
		return nil
	}
	params := map[string]any{"threadId": threadID}
	if turnID != "" {
		params["turnId"] = turnID
	}
	return c.wire.Call(ctx, "turn/interrupt", params, nil)
}

func (c *directNativeConnection) handleApproval(ctx context.Context, intent sessionpresentation.ApprovalIntent) error {
	c.mu.Lock()
	req := c.requests[intent.Request]
	if req == nil || req.approval == nil {
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
	decision := "decline"
	if intent.Choice >= 0 && intent.Choice < len(req.approval.Choices) {
		choice := req.approval.Choices[intent.Choice]
		switch {
		case choice.Approve && choice.Session:
			decision = "acceptForSession"
		case choice.Approve:
			decision = "accept"
		case choice.Deny:
			decision = "decline"
		}
	}
	delete(c.requests, intent.Request)
	c.requestOrder = slices.DeleteFunc(c.requestOrder, func(id string) bool { return id == intent.Request })
	c.publishLocked()
	c.mu.Unlock()

	if req.method == "item/permissions/requestApproval" {
		var payload struct {
			Permissions jsontext.Value `json:"permissions"`
		}
		_ = json.Unmarshal(req.rawParams, &payload)
		scope := "turn"
		if decision == "acceptForSession" {
			scope = "session"
		}
		if decision == "decline" || len(payload.Permissions) == 0 {
			return c.wire.Respond(ctx, req.id, map[string]any{
				"permissions": map[string]any{},
				"scope":       "turn",
			})
		}
		return c.wire.Respond(ctx, req.id, map[string]any{
			"permissions": payload.Permissions,
			"scope":       scope,
		})
	}
	if req.shellGuard {
		result := map[string]any{"decision": decision}
		if intent.Reason != "" {
			result["reason"] = intent.Reason
		}
		return c.wire.Respond(ctx, req.id, result)
	}
	result := map[string]any{"decision": decision}
	if intent.Reason != "" {
		result["reason"] = intent.Reason
	}
	return c.wire.Respond(ctx, req.id, result)
}

func (c *directNativeConnection) handleAnswers(ctx context.Context, in sessionapi.NativeAnswers) error {
	c.mu.Lock()
	req := c.requests[in.Call]
	if req == nil || req.question == nil {
		c.mu.Unlock()
		return sessionpresentation.ErrStaleIntent
	}
	delete(c.requests, in.Call)
	c.requestOrder = slices.DeleteFunc(c.requestOrder, func(id string) bool { return id == in.Call })
	c.publishLocked()
	c.mu.Unlock()

	if req.secret {
		secret := ""
		declined := in.Incomplete || len(in.Answers) == 0
		if len(in.Answers) > 0 {
			if in.Answers[0].Skip {
				declined = true
			}
			secret = in.Answers[0].Secret
			if secret == "" {
				secret = in.Answers[0].Draft.Text
			}
		}
		if declined || secret == "" {
			return c.wire.Respond(ctx, req.id, map[string]any{"decision": "decline"})
		}
		return c.wire.Respond(ctx, req.id, map[string]any{"decision": "provide", "secret": secret})
	}
	answers := make([]binding.QuestionAnswer, 0, len(in.Answers))
	for _, ans := range in.Answers {
		other := ans.Draft.Text
		if ans.Secret != "" {
			other = ans.Secret
		}
		answers = append(answers, binding.QuestionAnswer{
			ID:      ans.Question,
			Choice:  ans.Choice,
			Note:    ans.Note,
			Other:   other,
			Skipped: ans.Skip,
		})
	}
	return c.wire.Respond(ctx, req.id, binding.EncodeQuestionResponse(answers))
}

func (c *directNativeConnection) handleDisplay(ctx context.Context, intent sessionpresentation.DisplayIntent) error {
	if intent.Thread.ID == "" {
		return nil
	}
	c.mu.Lock()
	c.displayedThread = intent.Thread.ID
	if intent.Thread.Caller != "" {
		c.threadCallers[intent.Thread.ID] = intent.Thread.Caller
	}
	if intent.Thread.ID == c.mainThread || intent.Thread.ID == c.sideThread {
		c.inputThread = intent.Thread.ID
	}
	c.publishLocked()
	c.mu.Unlock()
	// Drive Herdr/Agents/Saved Changes panels from the attached frontend's selection.
	go func(threadID string) {
		c.reloadThreadTranscript(c.ctx, threadID)
		c.refreshChanges(c.ctx, threadID)
	}(intent.Thread.ID)
	return nil
}

func (c *directNativeConnection) handleResumeAction(ctx context.Context, action sessionpresentation.ResumeAction) error {
	switch action.Kind {
	case sessionpresentation.ResumeChoose:
		if action.Thread == "" {
			return nil
		}
		return c.resumeThread(ctx, action.Thread)
	case sessionpresentation.ResumePage:
		c.mu.Lock()
		all := c.resumeView != nil && c.resumeView.All
		cursor := c.resumeCursor
		c.mu.Unlock()
		if cursor == "" {
			return nil
		}
		return c.loadResumeList(ctx, cursor, all)
	case sessionpresentation.ResumeToggleWorkspace:
		c.mu.Lock()
		all := c.resumeView == nil || !c.resumeView.All
		c.mu.Unlock()
		return c.loadResumeList(ctx, "", all)
	case sessionpresentation.ResumeDismiss, sessionpresentation.ResumeCancel:
		c.mu.Lock()
		noMain := c.mainThread == ""
		c.resumeView = nil
		c.publishLocked()
		c.mu.Unlock()
		if noMain {
			c.finish(&sessionapi.NativeExit{Code: 0})
		}
		return nil
	}
	return nil
}

func (c *directNativeConnection) handleOutputDemand(demand sessionapi.NativeOutputDemand) {
	c.mu.Lock()
	defer c.mu.Unlock()
	results := make([]activity.OutputResult, 0, len(demand.Demands))
	threadID := c.mainThread
	if demand.Agent && c.displayedThread != "" {
		threadID = c.displayedThread
	}
	records := c.items[threadID]
	for _, d := range demand.Demands {
		for _, rec := range records {
			if rec.seq == d.Source {
				var lines []string
				if rec.output != "" {
					lines = strings.Split(rec.output, "\n")
				}
				results = append(results, activity.OutputResult{
					Demand: d,
					View:   activityfeed.OutputView{Lines: lines, Done: rec.entry.OutputComplete},
				})
				break
			}
		}
	}
	if demand.Agent {
		c.agentOutput = results
	} else {
		c.mainOutput = results
	}
	c.publishLocked()
}

func (c *directNativeConnection) handleDetailOutput(req sessionpresentation.DetailOutputRequest) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.details == nil || c.details.View.ID != req.ID {
		return
	}
	c.details.Output = &sessionpresentation.DetailOutput{Page: req.Page, Version: req.Version}
	c.publishLocked()
}
