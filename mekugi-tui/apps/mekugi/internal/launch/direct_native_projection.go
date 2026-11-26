package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	activityfeed "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
)

func (c *directNativeConnection) handleServerMessage(msg appserver.Message) {
	if len(msg.ID) > 0 && msg.Method != "" {
		c.handleServerRequest(msg)
		return
	}
	c.handleNotification(msg)
}

func (c *directNativeConnection) handleNotification(msg appserver.Message) {
	switch msg.Method {
	case "turn/started":
		var p struct {
			ThreadID string `json:"threadId"`
			Turn     struct {
				ID string `json:"id"`
			} `json:"turn"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			switch p.ThreadID {
			case c.btwThread:
				c.btwRunning = true
			case c.sideThread:
				c.sideActiveTurn = p.Turn.ID
				c.busy = true
			default:
				if p.ThreadID == "" || p.ThreadID == c.mainThread {
					c.activeTurn = p.Turn.ID
					c.busy = true
				}
			}
			c.publishLocked()
			c.mu.Unlock()
		}

	case "turn/completed":
		var p struct {
			ThreadID string `json:"threadId"`
			Turn     struct {
				ID     string `json:"id"`
				Status string `json:"status"`
				Error  *struct {
					Message string `json:"message"`
				} `json:"error"`
			} `json:"turn"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			switch p.ThreadID {
			case c.btwThread:
				c.btwRunning = false
			case c.sideThread:
				c.sideActiveTurn = ""
				c.busy = c.activeTurn != ""
			default:
				if p.ThreadID == "" || p.ThreadID == c.mainThread {
					c.activeTurn = ""
					c.busy = c.sideActiveTurn != ""
				}
			}
			if p.Turn.Error != nil && p.Turn.Error.Message != "" {
				c.setFeedbackLocked(p.Turn.Error.Message, true)
			}
			mainID := c.mainThread
			c.publishLocked()
			c.mu.Unlock()
			if mainID != "" {
				go c.refreshAuxiliary(c.ctx, mainID)
			}
		}

	case "item/started", "item/completed":
		var p struct {
			ThreadID string         `json:"threadId"`
			TurnID   string         `json:"turnId"`
			Item     jsontext.Value `json:"item"`
		}
		if json.Unmarshal(msg.Params, &p) == nil && len(p.Item) > 0 {
			complete := msg.Method == "item/completed"
			c.mu.Lock()
			threadID := p.ThreadID
			if threadID == "" {
				threadID = c.mainThread
			}
			itemType := c.upsertWireItemLocked(threadID, p.TurnID, p.Item, complete)
			mainID := c.mainThread
			c.publishLocked()
			c.mu.Unlock()
			if complete && mainID != "" {
				switch itemType {
				case "fileChange", "commandExecution":
					go c.refreshChanges(c.ctx, mainID)
				case "collabAgentToolCall", "subAgentActivity":
					go c.refreshSubtree(c.ctx, mainID)
				}
			}
		}

	case "item/agentMessage/delta":
		var p struct {
			ThreadID string `json:"threadId"`
			TurnID   string `json:"turnId"`
			ItemID   string `json:"itemId"`
			Delta    string `json:"delta"`
		}
		if json.Unmarshal(msg.Params, &p) == nil && p.Delta != "" {
			c.mu.Lock()
			if p.ThreadID != "" && p.ThreadID == c.btwThread {
				c.btwAnswer.WriteString(p.Delta)
			} else {
				threadID := p.ThreadID
				if threadID == "" {
					threadID = c.mainThread
				}
				rec := c.ensureItemLocked(threadID, p.TurnID, p.ItemID, "agentMessage", "final")
				rec.entry.Text += p.Delta
				rec.entry.Live = true
				rec.entry.Running = true
				refreshProjectedValues(rec)
				rec.entry.Revision++
			}
			c.publishLocked()
			c.mu.Unlock()
		}

	case "item/reasoning/summaryTextDelta", "item/reasoning/textDelta":
		var p struct {
			ThreadID string `json:"threadId"`
			TurnID   string `json:"turnId"`
			ItemID   string `json:"itemId"`
			Delta    string `json:"delta"`
		}
		if json.Unmarshal(msg.Params, &p) == nil && p.Delta != "" {
			c.mu.Lock()
			threadID := p.ThreadID
			if threadID == "" {
				threadID = c.mainThread
			}
			rec := c.ensureItemLocked(threadID, p.TurnID, p.ItemID, "reasoning", "reasoning")
			rec.entry.Text += p.Delta
			rec.entry.Live = true
			rec.entry.Running = true
			refreshProjectedValues(rec)
			rec.entry.Revision++
			c.publishLocked()
			c.mu.Unlock()
		}

	case "item/commandExecution/outputDelta", "item/fileChange/outputDelta":
		var p struct {
			ThreadID string `json:"threadId"`
			TurnID   string `json:"turnId"`
			ItemID   string `json:"itemId"`
			Delta    string `json:"delta"`
		}
		if json.Unmarshal(msg.Params, &p) == nil && p.Delta != "" {
			c.mu.Lock()
			threadID := p.ThreadID
			if threadID == "" {
				threadID = c.mainThread
			}
			rec := c.ensureItemLocked(threadID, p.TurnID, p.ItemID, "commandExecution", "tool")
			rec.output += p.Delta
			if len(rec.entry.Values) > 0 {
				rec.entry.Values[0].Output = 1
			}
			if rec.entry.Outputs == nil {
				rec.entry.Outputs = map[int]int{}
			}
			rec.entry.Outputs[1]++
			rec.entry.Revision++
			c.publishLocked()
			c.mu.Unlock()
		}

	case "thread/tokenUsage/updated":
		var p struct {
			ThreadID   string `json:"threadId"`
			TokenUsage struct {
				Total struct {
					TotalTokens           uint64 `json:"totalTokens"`
					InputTokens           uint64 `json:"inputTokens"`
					OutputTokens          uint64 `json:"outputTokens"`
					CachedInputTokens     uint64 `json:"cachedInputTokens"`
					CacheWriteInputTokens uint64 `json:"cacheWriteInputTokens"`
					ReasoningOutputTokens uint64 `json:"reasoningOutputTokens"`
				} `json:"total"`
				Last struct {
					TotalTokens uint64 `json:"totalTokens"`
				} `json:"last"`
				ModelContextWindow    uint64   `json:"modelContextWindow"`
				EstimatedCostUSD      *float64 `json:"estimatedCostUsd"`
				CostPartial           bool     `json:"costPartial"`
				OutputTokensPerSecond *float64 `json:"outputTokensPerSecond"`
				WorkDurationMs        *int64   `json:"workDurationMs"`
				Roundtrips            *int64   `json:"roundtrips"`
			} `json:"tokenUsage"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			target := p.ThreadID
			if target == "" {
				target = c.mainThread
			}
			if target != "" {
				if c.threadUsage == nil {
					c.threadUsage = make(map[string]directThreadUsage)
				}
				u := directThreadUsage{
					TotalTokens:           p.TokenUsage.Total.TotalTokens,
					InputTokens:           p.TokenUsage.Total.InputTokens,
					OutputTokens:          p.TokenUsage.Total.OutputTokens,
					CachedInputTokens:     p.TokenUsage.Total.CachedInputTokens,
					CacheWriteInputTokens: p.TokenUsage.Total.CacheWriteInputTokens,
					ReasoningOutputTokens: p.TokenUsage.Total.ReasoningOutputTokens,
					ContextTokens:         p.TokenUsage.Last.TotalTokens,
					ContextWindow:         p.TokenUsage.ModelContextWindow,
					CostPartial:           p.TokenUsage.CostPartial,
				}
				if p.TokenUsage.EstimatedCostUSD != nil {
					u.EstimatedCostUSD = *p.TokenUsage.EstimatedCostUSD
					u.CostKnown = true
				}
				if p.TokenUsage.OutputTokensPerSecond != nil {
					u.OutputTokensPerSecond = *p.TokenUsage.OutputTokensPerSecond
					u.HasThroughput = true
				}
				if p.TokenUsage.WorkDurationMs != nil && *p.TokenUsage.WorkDurationMs > 0 {
					u.WorkDurationMs = *p.TokenUsage.WorkDurationMs
				}
				if p.TokenUsage.Roundtrips != nil && *p.TokenUsage.Roundtrips > 0 {
					u.Roundtrips = uint64(*p.TokenUsage.Roundtrips)
				}
				c.threadUsage[target] = u
				if target == c.mainThread {
					c.usage = sessionapi.NativeUsage{
						TotalTokens:           u.TotalTokens,
						InputTokens:           u.InputTokens,
						OutputTokens:          u.OutputTokens,
						CachedInputTokens:     u.CachedInputTokens,
						ReasoningOutputTokens: u.ReasoningOutputTokens,
					}
					c.contextTokens = u.ContextTokens
					c.contextWindow = u.ContextWindow
				}
				c.publishLocked()
			}
			c.mu.Unlock()
		}

	case "account/rateLimits/updated":
		c.mu.Lock()
		if directObserveRateLimits(&c.rateLimits, msg.Params) == nil {
			c.publishLocked()
		}
		c.mu.Unlock()

	case "thread/name/updated":
		var p struct {
			ThreadID   string `json:"threadId"`
			ThreadName string `json:"threadName"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			if p.ThreadID == c.mainThread && p.ThreadName != "" {
				c.title = p.ThreadName
				c.publishLocked()
			}
			c.mu.Unlock()
		}

	case "thread/settings/updated":
		var p struct {
			ThreadID        string `json:"threadId"`
			Model           string `json:"model"`
			Effort          string `json:"effort"`
			ReasoningEffort string `json:"reasoningEffort"`
			ServiceTier     string `json:"serviceTier"`
			ThreadSettings  *struct {
				Model       string  `json:"model"`
				Effort      *string `json:"effort"`
				ServiceTier *string `json:"serviceTier"`
			} `json:"threadSettings"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			if p.ThreadID == "" || p.ThreadID == c.mainThread {
				if s := p.ThreadSettings; s != nil {
					if s.Model != "" {
						c.model = s.Model
					}
					if s.Effort != nil {
						c.effort = *s.Effort
					} else {
						c.effort = ""
					}
					if s.ServiceTier != nil {
						c.tier = *s.ServiceTier
					} else {
						c.tier = ""
					}
				} else {
					if p.Model != "" {
						c.model = p.Model
					}
					if effort := firstNonEmpty(p.Effort, p.ReasoningEffort); effort != "" {
						c.effort = effort
					}
					if p.ServiceTier != "" {
						c.tier = p.ServiceTier
					}
				}
				c.publishLocked()
			}
			c.mu.Unlock()
		}

	case "skills/changed":
		c.mu.Lock()
		c.skillsGen++
		c.skillsLoaded = false
		c.skills = nil
		c.mu.Unlock()

	case "thread/queue/changed":
		var p struct {
			ThreadID string `json:"threadId"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			target := p.ThreadID
			if target == "" {
				target = c.mainThread
			}
			isMain := target != "" && target == c.mainThread
			c.mu.Unlock()
			if isMain && c.wire != nil {
				go c.refreshQueue(c.ctx, target)
			}
		}

	case "thread/prediction/updated":
		var p struct {
			ThreadID   string `json:"threadId"`
			TurnID     string `json:"turnId"`
			Prediction string `json:"prediction"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			c.mu.Lock()
			if p.ThreadID == "" || p.ThreadID == c.mainThread {
				c.prediction = p.Prediction
				c.predictionTurn = p.TurnID
				c.publishLocked()
			}
			c.mu.Unlock()
		}

	case "thread/reverted":
		var p struct {
			ThreadID string `json:"threadId"`
		}
		if json.Unmarshal(msg.Params, &p) == nil {
			target := p.ThreadID
			if target == "" {
				c.mu.Lock()
				target = c.mainThread
				c.mu.Unlock()
			}
			if target != "" {
				go func() {
					c.reloadThreadTranscript(c.ctx, target)
					c.refreshChanges(c.ctx, target)
				}()
			}
		}

	case "thread/started", "thread/status/changed":
		c.mu.Lock()
		mainID := c.mainThread
		c.mu.Unlock()
		if mainID != "" {
			go c.refreshSubtree(c.ctx, mainID)
		}

	case "serverRequest/resolved":
		if resolved, err := binding.DecodeResolvedRequest(msg.Params); err == nil {
			key := string(resolved.RequestID)
			c.mu.Lock()
			if _, exists := c.requests[key]; exists {
				delete(c.requests, key)
				c.requestOrder = slices.DeleteFunc(c.requestOrder, func(id string) bool { return id == key })
				c.publishLocked()
			}
			c.mu.Unlock()
		}
	}
}

func (c *directNativeConnection) handleServerRequest(msg appserver.Message) {
	key := string(msg.ID)
	req := &directPendingServerRequest{
		id:        slices.Clone(msg.ID),
		method:    msg.Method,
		rawParams: slices.Clone(msg.Params),
	}

	switch msg.Method {
	case "item/commandExecution/requestApproval", "item/fileChange/requestApproval", "item/permissions/requestApproval", "shellGuard/requestApproval":
		req.shellGuard = msg.Method == "shellGuard/requestApproval"
		view := decodeDirectApprovalView(key, msg.Method, msg.Params)
		req.threadID = view.Thread
		req.turnID = view.Turn
		req.itemID = view.Item
		req.approval = &view

	case "item/tool/requestUserInput":
		call := decodeDirectQuestionCall(key, msg.Params)
		req.threadID = call.Thread
		req.turnID = call.Turn
		req.itemID = call.Item
		req.question = &call

	case "shellGuard/requestSecret":
		req.secret = true
		req.shellGuard = true
		call := decodeDirectSecretCall(key, msg.Params)
		req.threadID = call.Thread
		req.question = &call

	default:
		_ = c.wire.RespondError(c.ctx, msg.ID, -32601, "unsupported terminal client server request: "+msg.Method)
		return
	}

	c.mu.Lock()
	if _, exists := c.requests[key]; !exists {
		c.requestOrder = append(c.requestOrder, key)
	}
	c.requests[key] = req
	c.publishLocked()
	c.mu.Unlock()
}

func decodeDirectApprovalView(id, method string, params jsontext.Value) prompt.ApprovalView {
	view := prompt.ApprovalView{
		ID: id,
		Choices: []prompt.Choice{
			{ID: 0, Label: "Allow", Approve: true},
			{ID: 1, Label: "Allow for session", Approve: true, Session: true, Lasting: true},
			{ID: 2, Label: "Deny", Deny: true},
		},
	}
	if decoded, err := binding.DecodeApprovalRequest(params); err == nil {
		view.Thread = decoded.ThreadID
		view.Turn = decoded.TurnID
		view.Item = decoded.ItemID
		view.Subject = decoded.Command
		if decoded.Reason != "" {
			view.Details = append(view.Details, decoded.Reason)
		}
		if decoded.GrantRoot != "" {
			view.Details = append(view.Details, "Grant root: "+decoded.GrantRoot)
		}
	}
	switch method {
	case "item/commandExecution/requestApproval":
		view.Title = "Approve command"
	case "item/fileChange/requestApproval":
		view.Title = "Approve file change"
	case "item/permissions/requestApproval":
		view.Title = "Approve permissions"
		var permEnv struct {
			Permissions jsontext.Value `json:"permissions"`
		}
		if json.Unmarshal(params, &permEnv) == nil && len(permEnv.Permissions) > 0 {
			var profile struct {
				FileSystem *struct {
					Read  []string `json:"read"`
					Write []string `json:"write"`
				} `json:"fileSystem"`
				Network *struct {
					Enabled bool `json:"enabled"`
				} `json:"network"`
			}
			added := false
			if json.Unmarshal(permEnv.Permissions, &profile) == nil {
				if fs := profile.FileSystem; fs != nil {
					if len(fs.Read) > 0 {
						view.Details = append(view.Details, "Read: "+strings.Join(fs.Read, ", "))
						added = true
					}
					if len(fs.Write) > 0 {
						view.Details = append(view.Details, "Write: "+strings.Join(fs.Write, ", "))
						added = true
					}
				}
				if net := profile.Network; net != nil && net.Enabled {
					view.Details = append(view.Details, "Network access: enabled")
					added = true
				}
			}
			if !added && string(permEnv.Permissions) != "{}" && string(permEnv.Permissions) != "null" {
				view.Details = append(view.Details, "Permissions: "+string(permEnv.Permissions))
			}
		}
	case "shellGuard/requestApproval":
		view.Title = "Approve guarded shell command"
		var sg struct {
			ThreadID   string   `json:"threadId"`
			TurnID     string   `json:"turnId"`
			ItemID     string   `json:"itemId"`
			Kind       string   `json:"kind"`
			Cwd        string   `json:"cwd"`
			Executable string   `json:"executable"`
			Argv       []string `json:"argv"`
			Command    string   `json:"command"`
			Summary    string   `json:"summary"`
			Reason     string   `json:"reason"`
		}
		if json.Unmarshal(params, &sg) == nil {
			if sg.ThreadID != "" {
				view.Thread = sg.ThreadID
			}
			if sg.TurnID != "" {
				view.Turn = sg.TurnID
			}
			if sg.ItemID != "" {
				view.Item = sg.ItemID
			}
			if cmd := formatShellGuardCommand(sg.Executable, sg.Argv); cmd != "" {
				view.Subject = cmd
			} else if sg.Command != "" {
				view.Subject = sg.Command
			}
			if sg.Kind != "" {
				view.Details = append(view.Details, "Kind: "+sg.Kind)
			}
			if sg.Cwd != "" {
				view.Details = append(view.Details, "Directory: "+sg.Cwd)
			}
			if sg.Summary != "" {
				view.Details = append(view.Details, sg.Summary)
			}
			if sg.Reason != "" {
				view.Details = append(view.Details, sg.Reason)
			}
		}
	}
	return view
}

func formatShellGuardCommand(executable string, argv []string) string {
	if executable == "" && len(argv) == 0 {
		return ""
	}
	parts := make([]string, 0, len(argv)+1)
	if executable != "" {
		if len(argv) > 0 && (argv[0] == executable || argv[0] == filepath.Base(executable)) {
			parts = append(parts, argv...)
		} else {
			parts = append(parts, executable)
			parts = append(parts, argv...)
		}
	} else {
		parts = append(parts, argv...)
	}
	return strings.Join(parts, " ")
}

func decodeDirectQuestionCall(id string, params jsontext.Value) prompt.CallView {
	call := prompt.CallView{ID: id, Sync: true}
	if decoded, err := binding.DecodeQuestionRequest(params); err == nil {
		call.Thread = decoded.ThreadID
		call.Turn = decoded.TurnID
		call.Item = decoded.ItemID
		for _, q := range decoded.Questions {
			qv := prompt.QuestionView{
				ID:     q.ID,
				Header: q.Header,
				Text:   q.Question,
				Secret: q.IsSecret,
			}
			for _, opt := range q.Options {
				qv.Choices = append(qv.Choices, prompt.Option{Label: opt.Label, Description: opt.Description})
			}
			call.Questions = append(call.Questions, qv)
		}
	}
	return call
}

func decodeDirectSecretCall(id string, params jsontext.Value) prompt.CallView {
	var sg struct {
		ThreadID   string   `json:"threadId"`
		TurnID     string   `json:"turnId"`
		ItemID     string   `json:"itemId"`
		Kind       string   `json:"kind"`
		Cwd        string   `json:"cwd"`
		Executable string   `json:"executable"`
		Argv       []string `json:"argv"`
		Prompt     string   `json:"prompt"`
		Command    string   `json:"command"`
	}
	_ = json.Unmarshal(params, &sg)
	text := sg.Prompt
	if text == "" {
		switch sg.Kind {
		case "sudoPassword":
			text = "Enter sudo password"
		case "gpgPassphrase":
			text = "Enter GPG passphrase"
		default:
			text = "Enter secret"
		}
	}
	cmd := formatShellGuardCommand(sg.Executable, sg.Argv)
	if cmd == "" {
		cmd = sg.Command
	}
	if cmd != "" {
		text += " (" + cmd + ")"
	}
	return prompt.CallView{
		ID:     id,
		Sync:   true,
		Thread: sg.ThreadID,
		Turn:   sg.TurnID,
		Item:   sg.ItemID,
		Questions: []prompt.QuestionView{{
			ID:     "secret",
			Header: "Secret required",
			Text:   text,
			Secret: true,
		}},
	}
}

func (c *directNativeConnection) ensureItemLocked(threadID, turnID, itemID, itemType, kind string) *directItemRecord {
	if itemID != "" {
		if rec, ok := c.itemsByID[itemID]; ok {
			return rec
		}
	}
	c.seq++
	caller := c.threadCallers[threadID]
	if caller == "" {
		caller = "/root"
	}
	rec := &directItemRecord{
		id:       itemID,
		threadID: threadID,
		turnID:   turnID,
		itemType: itemType,
		seq:      c.seq,
		entry: activity.Entry{
			Facts: activity.Facts{
				Seq:      c.seq,
				Agent:    caller,
				Kind:     kind,
				Observed: time.Now(),
			},
			Revision: 1,
		},
	}
	c.items[threadID] = append(c.items[threadID], rec)
	if itemID != "" {
		c.itemsByID[itemID] = rec
	}
	return rec
}

func (c *directNativeConnection) hydrateThreadTurnsLocked(threadID string, turns []directWireTurn) {
	preservedSeq := make(map[string]uint64, len(c.items[threadID]))
	for _, rec := range c.items[threadID] {
		if rec.id != "" {
			preservedSeq[rec.id] = rec.seq
			delete(c.itemsByID, rec.id)
		}
	}
	c.items[threadID] = nil
	for _, turn := range turns {
		for _, rawItem := range turn.Items {
			c.upsertWireItemLocked(threadID, turn.ID, rawItem, true)
		}
	}
	for _, rec := range c.items[threadID] {
		rec.entry.Restored = true
		if prevSeq, ok := preservedSeq[rec.id]; ok {
			rec.seq = prevSeq
			rec.entry.Seq = prevSeq
		}
	}
}

// upsertWireItemLocked maps native mekugi-app-server ThreadItem payloads
// directly into activity.Entry publications. Unknown / server-owned inline card
// item payloads render directly from their server fields without Go backend state.
func (c *directNativeConnection) upsertWireItemLocked(threadID, turnID string, raw jsontext.Value, complete bool) string {
	var fields map[string]jsontext.Value
	if err := json.Unmarshal(raw, &fields); err != nil {
		return ""
	}
	itemType := decodeJSONString(fields["type"])
	itemID := decodeJSONString(fields["id"])
	if threadID == c.btwThread && itemType == "agentMessage" {
		if text := decodeJSONString(fields["text"]); text != "" {
			c.btwAnswer.Reset()
			c.btwAnswer.WriteString(text)
		}
		return itemType
	}

	kind := "tool"
	switch itemType {
	case "userMessage":
		kind = "user"
	case "agentMessage":
		kind = "final"
	case "reasoning":
		kind = "reasoning"
	case "plan":
		kind = "plan"
	case "contextCompaction":
		kind = "compaction"
	}

	rec := c.ensureItemLocked(threadID, turnID, itemID, itemType, kind)
	rec.entry.Kind = kind
	rec.entry.Live = !complete
	rec.entry.Running = !complete
	rec.entry.ItemComplete = complete
	if complete {
		rec.entry.Completed = time.Now()
		rec.entry.OutputComplete = true
	}

	switch itemType {
	case "userMessage":
		rec.entry.Text = extractUserMessageText(fields["content"])
	case "agentMessage":
		if text := decodeJSONString(fields["text"]); text != "" {
			rec.entry.Text = text
		}
		refreshProjectedValues(rec)
	case "plan":
		rec.entry.Text = decodeJSONString(fields["text"])
		rec.entry.Values = []activity.Value{{
			Content: activityfeed.Content{
				Kind: "message",
				Verb: "Plan",
				Body: rec.entry.Text,
			},
		}}
		activity.Identify(rec.entry.Values, rec.entry.Seq)
	case "reasoning":
		var summary, content []string
		_ = json.Unmarshal(fields["summary"], &summary)
		_ = json.Unmarshal(fields["content"], &content)
		joined := strings.Join(summary, "\n")
		if joined == "" {
			joined = strings.Join(content, "\n")
		}
		if joined != "" {
			rec.entry.Text = joined
		}
		refreshProjectedValues(rec)
	case "commandExecution":
		command := unwrapDirectShellCommand(decodeJSONString(fields["command"]))
		status := decodeJSONString(fields["status"])
		cwd := decodeJSONString(fields["cwd"])
		agg := decodeJSONString(fields["aggregatedOutput"])
		if agg != "" {
			rec.output = agg
		}
		running := status == "inProgress"
		exited := status != "" && !running
		var exitCode int
		if raw, ok := fields["exitCode"]; ok && len(raw) > 0 {
			_ = json.Unmarshal(raw, &exitCode)
		}
		var durationMs int64
		if raw, ok := fields["durationMs"]; ok && len(raw) > 0 {
			_ = json.Unmarshal(raw, &durationMs)
		}
		var tail []string
		if rec.output != "" {
			trimmed := strings.TrimSuffix(rec.output, "\n")
			tail = strings.Split(trimmed, "\n")
		}
		val := activity.Value{
			Content: activityfeed.Content{
				Kind:   "op",
				Verb:   "Run",
				Code:   command,
				Lang:   "bash",
				Fenced: strings.Contains(command, "\n"),
			},
			Workdir:  cwd,
			Running:  running,
			Exited:   exited,
			ExitCode: exitCode,
			Duration: time.Duration(durationMs) * time.Millisecond,
			Tail:     tail,
		}
		if rec.output != "" {
			val.Output = 1
			if rec.entry.Outputs == nil {
				rec.entry.Outputs = map[int]int{}
			}
			rec.entry.Outputs[1]++
		}
		rec.entry.Values = []activity.Value{val}
		activity.Identify(rec.entry.Values, rec.entry.Seq)
		rec.entry.Outcome = status
		rec.entry.OutputComplete = exited
		if exited && rec.entry.Completed.IsZero() {
			rec.entry.Completed = time.Now()
		}
	case "fileChange":
		var changeList []struct {
			Path string         `json:"path"`
			Kind jsontext.Value `json:"kind"`
			Diff string         `json:"diff"`
		}
		_ = json.Unmarshal(fields["changes"], &changeList)
		status := decodeJSONString(fields["status"])
		running := status == "inProgress"
		exited := status != "" && !running
		vals := make([]activity.Value, 0, len(changeList))
		for _, ch := range changeList {
			verb, label := decodeFileChangeKind(ch.Kind, ch.Path)
			var tail []string
			if ch.Diff != "" {
				tail = strings.Split(strings.TrimSuffix(ch.Diff, "\n"), "\n")
			}
			vals = append(vals, activity.Value{
				Content: activityfeed.Content{
					Kind:  "op",
					Verb:  verb,
					Label: label,
					Code:  ch.Diff,
					Lang:  "diff",
				},
				Running: running,
				Exited:  exited,
				Tail:    tail,
			})
		}
		rec.entry.Values = vals
		activity.Identify(rec.entry.Values, rec.entry.Seq)
		rec.entry.Outcome = status
		rec.entry.OutputComplete = exited
		if exited && rec.entry.Completed.IsZero() {
			rec.entry.Completed = time.Now()
		}
	case "mcpToolCall", "dynamicToolCall":
		tool := decodeJSONString(fields["tool"])
		server := decodeJSONString(fields["server"])
		label := tool
		if server != "" && tool != "" {
			label = server + "/" + tool
		}
		rec.entry.Values = []activity.Value{{
			Content: activityfeed.Content{
				Kind:  "op",
				Verb:  "Call",
				Label: label,
			},
		}}
		activity.Identify(rec.entry.Values, rec.entry.Seq)
		rec.entry.Outcome = decodeJSONString(fields["status"])
	case "collabAgentToolCall":
		tool := decodeJSONString(fields["tool"])
		promptText := decodeJSONString(fields["prompt"])
		rec.entry.Values = []activity.Value{{
			Content: activityfeed.Content{
				Kind:  "op",
				Verb:  tool,
				Label: promptText,
			},
		}}
		activity.Identify(rec.entry.Values, rec.entry.Seq)
		rec.entry.Outcome = decodeJSONString(fields["status"])
	case "subAgentActivity":
		agentPath := decodeJSONString(fields["agentPath"])
		assignment := decodeJSONString(fields["assignment"])
		rec.entry.Assignment = assignment != ""
		rec.entry.AssignmentTo = agentPath
		rec.entry.AssignmentText = assignment
		rec.entry.Text = assignment
		rec.entry.Values = []activity.Value{{
			To: agentPath,
			Content: activityfeed.Content{
				Kind: "message",
				Body: assignment,
			},
		}}
		activity.Identify(rec.entry.Values, rec.entry.Seq)
	case "contextCompaction":
		rec.entry.ContextBoundary = true
		rec.entry.Text = "Context compacted"
		refreshProjectedValues(rec)
	default:
		// Render server-owned inline cards and future ThreadItem models directly
		// from their server-published fields without adding Go backend state.
		title := firstNonEmpty(
			decodeJSONString(fields["title"]),
			decodeJSONString(fields["header"]),
			decodeJSONString(fields["label"]),
			itemType,
		)
		body := firstNonEmpty(
			decodeJSONString(fields["body"]),
			decodeJSONString(fields["markdown"]),
			decodeJSONString(fields["text"]),
			decodeJSONString(fields["summary"]),
		)
		rec.entry.Text = body
		rec.entry.Values = []activity.Value{{
			Content: activityfeed.Content{
				Kind: "message",
				Verb: title,
				Body: body,
			},
		}}
		activity.Identify(rec.entry.Values, rec.entry.Seq)
		rec.entry.Outcome = decodeJSONString(fields["status"])
	}
	rec.entry.Revision++
	return itemType
}

func refreshProjectedValues(rec *directItemRecord) {
	content := activity.ItemContent{Facts: rec.entry.Facts}
	if rec.entry.Kind == "reasoning" {
		content.Details = &activity.ItemDetails{ReasoningLive: rec.entry.Live}
	}
	rec.entry.Values = content.Values()
}

func extractUserMessageText(raw jsontext.Value) string {
	var parts []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if err := json.Unmarshal(raw, &parts); err != nil {
		return ""
	}
	var out []string
	for _, part := range parts {
		if part.Text != "" {
			out = append(out, part.Text)
		}
	}
	return strings.Join(out, "\n")
}

func decodeJSONString(raw jsontext.Value) string {
	if len(raw) == 0 {
		return ""
	}
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s
	}
	return ""
}

func unwrapDirectShellCommand(raw string) string {
	trimmed := strings.TrimSpace(raw)
	fields := strings.SplitN(trimmed, " ", 3)
	if len(fields) != 3 {
		return raw
	}
	shell := fields[0]
	if idx := strings.LastIndex(shell, "/"); idx >= 0 {
		shell = shell[idx+1:]
	}
	if shell != "bash" && shell != "sh" && shell != "zsh" && shell != "dash" {
		return raw
	}
	if fields[1] != "-lc" && fields[1] != "-c" {
		return raw
	}
	arg := strings.TrimSpace(fields[2])
	if len(arg) >= 2 && arg[0] == '\'' && arg[len(arg)-1] == '\'' {
		return strings.ReplaceAll(arg[1:len(arg)-1], "'\\''", "'")
	}
	if len(arg) >= 2 && arg[0] == '"' && arg[len(arg)-1] == '"' {
		inner := arg[1 : len(arg)-1]
		inner = strings.ReplaceAll(inner, "\\\"", "\"")
		inner = strings.ReplaceAll(inner, "\\\\", "\\")
		inner = strings.ReplaceAll(inner, "\\$", "$")
		inner = strings.ReplaceAll(inner, "\\`", "`")
		return inner
	}
	if !strings.ContainsAny(arg, " \t\r\n") {
		return arg
	}
	return raw
}

func decodeFileChangeKind(raw jsontext.Value, path string) (verb, label string) {
	label = path
	if len(raw) == 0 {
		return "Edit", label
	}
	var str string
	if err := json.Unmarshal(raw, &str); err == nil && str != "" {
		return strings.ToUpper(str[:1]) + str[1:], label
	}
	var obj struct {
		Type     string `json:"type"`
		MovePath string `json:"move_path"`
	}
	if err := json.Unmarshal(raw, &obj); err == nil {
		switch obj.Type {
		case "add":
			return "Create", label
		case "delete":
			return "Delete", label
		case "update":
			if obj.MovePath != "" {
				return "Move", path + " => " + obj.MovePath
			}
			return "Edit", label
		}
	}
	return "Edit", label
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

func (c *directNativeConnection) refreshSubtree(ctx context.Context, rootThread string) {
	var resp struct {
		Data []directWireThread `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/subtree/list", map[string]any{"threadId": rootThread}, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	var threads []string
	var roster []activity.AgentRow
	for i, th := range resp.Data {
		threads = append(threads, th.ID)
		name := th.AgentPath
		if name == "" {
			if th.ID == rootThread {
				name = "/root"
			} else {
				name = fmt.Sprintf("/root/agent-%d", i)
			}
		}
		c.threadCallers[th.ID] = name
		depth := strings.Count(strings.TrimPrefix(name, "/"), "/")
		if th.ID == rootThread {
			depth = 0
		}
		var statusObj struct {
			Type string `json:"type"`
		}
		_ = json.Unmarshal(th.Status, &statusObj)
		responding := statusObj.Type == "active"
		final := statusObj.Type == "idle" && th.ID != rootThread
		roster = append(roster, activity.AgentRow{
			Agent: activity.Agent{
				AgentFacts: activity.AgentFacts{
					Name:       name,
					Role:       th.AgentRole,
					Thread:     th.ID,
					Responding: responding,
					Final:      final,
				},
			},
			Depth: depth,
		})
	}
	c.family = &sessionapi.Family{
		Root:    rootThread,
		Threads: threads,
		Agents:  &activity.Publication{Roster: roster},
		Status:  sessionpresentation.Label{Text: c.statusTextLocked(), Running: c.busy},
	}
	c.publishLocked()
}

func (c *directNativeConnection) refreshChanges(ctx context.Context, threadID string) {
	var resp directThreadChangesResponse
	if err := c.wire.Call(ctx, "thread/changes/read", map[string]any{"threadId": threadID}, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.reviewRev++
	files := make([]changes.File, 0, len(resp.Files))
	for i, f := range resp.Files {
		beforePath := f.Path
		if f.MovedFrom != "" {
			beforePath = f.MovedFrom
		}
		if f.Status == "A" {
			beforePath = ""
		}
		afterPath := f.Path
		if f.Status == "D" {
			afterPath = ""
		}
		reviewFile := diffmodel.ReviewFile{
			BeforePath: beforePath,
			AfterPath:  afterPath,
			Diff:       f.UnifiedDiff,
			Binary:     f.IsBinary,
		}
		if len(f.CoverageNotes) > 0 {
			reviewFile.Incomplete = strings.Join(f.CoverageNotes, "; ")
		}
		chunk := changes.Chunk{
			Key:          fmt.Sprintf("native-%d-%s", i, f.Path),
			Status:       f.Status,
			Workspace:    resp.Workspace,
			CaptureOrder: uint64(i + 1),
			Review:       reviewFile,
			Origin: changes.Origin{
				Caller: c.threadCallers[threadID],
				Source: "native",
			},
		}
		files = append(files, changes.File{
			Path:   f.Path,
			Chunks: []changes.Chunk{chunk},
			Status: diffmodel.StatusOf(reviewFile),
		})
	}
	var pub changes.Publication
	pub = pub.With(files)
	ws := resp.Workspace
	if ws == "" {
		ws = c.workspace
	}
	c.review = &changes.ReviewView{
		Publication: pub,
		Workspace:   ws,
		Revision:    c.reviewRev,
	}
	c.publishLocked()
}

func (c *directNativeConnection) refreshQueue(ctx context.Context, threadID string) {
	var resp struct {
		Data []struct {
			ID    string         `json:"id"`
			Input jsontext.Value `json:"input"`
		} `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/queue/list", map[string]any{"threadId": threadID}, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if threadID != c.mainThread {
		return
	}
	c.queue = make([]conversation.PendingText, 0, len(resp.Data))
	for _, item := range resp.Data {
		text := extractUserMessageText(item.Input)
		if text == "" {
			text = "(queued input)"
		}
		c.queue = append(c.queue, conversation.PendingText{Text: text})
	}
	c.publishLocked()
}

type directRateLimitWindow struct {
	Used    float64 `json:"usedPercent"`
	Minutes *int64  `json:"windowDurationMins"`
	Resets  *int64  `json:"resetsAt"`
}

func (w directRateLimitWindow) label(position int) string {
	if w.Minutes == nil {
		return []string{"primary", "secondary"}[position]
	}
	switch minutes := *w.Minutes; {
	case minutes == 10080:
		return "weekly"
	case minutes > 0 && minutes%60 == 0:
		return fmt.Sprintf("%dh", minutes/60)
	default:
		return fmt.Sprintf("%dm", minutes)
	}
}

type directRateLimitSnapshot struct {
	ID        *string                `json:"limitId"`
	Name      string                 `json:"limitName"`
	Primary   *directRateLimitWindow `json:"primary"`
	Secondary *directRateLimitWindow `json:"secondary"`
}

func directObserveRateLimitSnapshot(windows *[2]*sessionpresentation.CaptionLimit, snapshot directRateLimitSnapshot) {
	if windows == nil {
		return
	}
	for i, w := range []*directRateLimitWindow{snapshot.Primary, snapshot.Secondary} {
		if w != nil {
			windows[i] = &sessionpresentation.CaptionLimit{
				Label: w.label(i),
				Used:  max(0, min(100, w.Used)),
			}
		}
	}
}

func directObserveRateLimits(windows *[2]*sessionpresentation.CaptionLimit, params []byte) error {
	var update struct {
		RateLimits directRateLimitSnapshot `json:"rateLimits"`
	}
	if err := json.Unmarshal(params, &update); err != nil {
		return err
	}
	if id := update.RateLimits.ID; id != nil && *id != "" && *id != "codex" {
		return nil
	}
	directObserveRateLimitSnapshot(windows, update.RateLimits)
	return nil
}

func directCaptionLimits(windows [2]*sessionpresentation.CaptionLimit) []sessionpresentation.CaptionLimit {
	var limits []sessionpresentation.CaptionLimit
	for _, w := range windows {
		if w != nil {
			limits = append(limits, *w)
		}
	}
	return limits
}

func (c *directNativeConnection) refreshRateLimits(ctx context.Context) {
	if c.wire == nil {
		return
	}
	var resp struct {
		RateLimits directRateLimitSnapshot            `json:"rateLimits"`
		ByID       map[string]directRateLimitSnapshot `json:"rateLimitsByLimitId"`
	}
	if err := c.wire.Call(ctx, "account/rateLimits/read", nil, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if codex, ok := resp.ByID["codex"]; ok {
		directObserveRateLimitSnapshot(&c.rateLimits, codex)
	} else {
		directObserveRateLimitSnapshot(&c.rateLimits, resp.RateLimits)
	}
	c.publishLocked()
}

func (c *directNativeConnection) reloadThreadTranscript(ctx context.Context, threadID string) {
	var resp struct {
		Thread directWireThread `json:"thread"`
	}
	if err := c.wire.Call(ctx, "thread/read", map[string]any{"threadId": threadID, "includeTurns": true}, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.hydrateThreadTurnsLocked(threadID, resp.Thread.Turns)
	c.publishLocked()
}

func (c *directNativeConnection) loadResumeList(ctx context.Context, cursor string, all bool) error {
	params := map[string]any{"limit": 50}
	if cursor != "" {
		params["cursor"] = cursor
	}
	if !all && c.workspace != "" {
		params["cwd"] = c.workspace
	}
	var resp struct {
		Data       []directWireThread `json:"data"`
		NextCursor string             `json:"nextCursor"`
	}
	if err := c.wire.Call(ctx, "thread/list", params, &resp); err != nil {
		return fmt.Errorf("list native threads: %w", err)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	var rows []sessionpresentation.ResumeRow
	if cursor != "" && c.resumeView != nil {
		rows = slices.Clone(c.resumeView.Rows)
	}
	for _, th := range resp.Data {
		if !all && c.workspace != "" && th.Cwd != "" && th.Cwd != c.workspace {
			continue
		}
		title := th.Name
		if title == "" {
			title = th.Preview
		}
		rows = append(rows, sessionpresentation.ResumeRow{
			ID:        th.ID,
			Title:     title,
			Workspace: th.Cwd,
			Updated:   th.UpdatedAt,
		})
	}
	c.resumeCursor = resp.NextCursor
	id := uint64(1)
	if c.resumeView != nil {
		id = c.resumeView.ID + 1
	}
	c.resumeView = &sessionpresentation.ResumeView{
		ID:        id,
		Workspace: c.workspace,
		Startup:   c.mainThread == "",
		All:       all,
		HasMore:   resp.NextCursor != "",
		CanPage:   resp.NextCursor != "",
		PageAfter: resp.NextCursor,
		Rows:      rows,
		Scanned:   len(rows),
		LoadedAt:  time.Now(),
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) setFeedbackLocked(text string, alert bool) {
	c.feedbackText = text
	c.feedbackAlert = alert
	c.feedbackRev++
}
