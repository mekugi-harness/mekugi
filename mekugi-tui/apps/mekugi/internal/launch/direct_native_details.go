package launch

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activityfeed "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
	usagepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/presentation"
)

const maxDetailFileBytes = 512 * 1024

func clonePanelView(p *sessionpresentation.PanelView) *sessionpresentation.PanelView {
	if p == nil {
		return nil
	}
	cp := *p
	cp.Fields = slices.Clone(p.Fields)
	cp.MetricViews = slices.Clone(p.MetricViews)
	return &cp
}

func (c *directNativeConnection) findRecordBySeqLocked(threadID string, seq uint64) *directItemRecord {
	if seq == 0 {
		return nil
	}
	if threadID != "" {
		for _, rec := range c.items[threadID] {
			if rec.seq == seq {
				return rec
			}
		}
	}
	for _, records := range c.items {
		for _, rec := range records {
			if rec.seq == seq {
				return rec
			}
		}
	}
	return nil
}

func (c *directNativeConnection) handleOpenDetails(ctx context.Context, req sessionapi.NativeDetailRequest) error {
	switch req.Kind {
	case sessionapi.NativeFileDetails:
		return c.openFileDetails(req)
	case sessionapi.NativeSkillDetails:
		return c.openSkillDetails(ctx, req)
	case sessionapi.NativeProcessDetails:
		return c.openProcessDetails(ctx, req)
	case sessionapi.NativeErrorDetails:
		c.mu.Lock()
		defer c.mu.Unlock()
		return c.openErrorDetailsLocked(req, "")
	case sessionapi.NativeRunDetails:
		c.mu.Lock()
		defer c.mu.Unlock()
		return c.openEntryDetailsLocked(req, true)
	default:
		c.mu.Lock()
		defer c.mu.Unlock()
		return c.openEntryDetailsLocked(req, false)
	}
}

func (c *directNativeConnection) openEntryDetailsLocked(req sessionapi.NativeDetailRequest, combinedRun bool) error {
	threadID := req.Displayed.ID
	if threadID == "" {
		if req.Agent && c.displayedThread != "" {
			threadID = c.displayedThread
		} else {
			threadID = c.mainThread
		}
	}
	sources := req.Sources
	if len(sources) == 0 && req.Source != 0 {
		sources = []uint64{req.Source}
	}
	var values []activity.Value
	for _, seq := range sources {
		rec := c.findRecordBySeqLocked(threadID, seq)
		if rec == nil {
			continue
		}
		values = append(values, activity.CloneValues(rec.entry.Values)...)
	}
	if len(values) == 0 && req.Path != "" {
		if rec, ok := c.itemsByID[req.Path]; ok {
			values = append(values, activity.CloneValues(rec.entry.Values)...)
		}
	}
	if len(values) == 0 {
		return errors.New("detail target not found")
	}
	pages := activity.DetailPages(values)
	if len(pages) == 0 {
		return errors.New("no detail pages available")
	}
	items := make([]sessionpresentation.DetailItem, 0, len(pages))
	selected := pages[0].ID
	var selectedRec *directItemRecord
	var selectedPage activity.DetailPage
	for _, page := range pages {
		if combinedRun {
			page.CombinedOutput = true
		}
		rec := c.findRecordBySeqLocked(threadID, page.Source)
		var outVer uint64
		retained := false
		outputLive := false
		if rec != nil {
			outVer = rec.entry.Revision
			if rec.output != "" || len(page.Tail) > 0 {
				retained = true
			}
			outputLive = rec.entry.Running
		}
		items = append(items, sessionpresentation.DetailItem{
			DetailPage:    page,
			OutputVersion: outVer,
			Retained:      retained,
			OutputLive:    outputLive,
		})
		if (req.Operation != 0 && page.Operation == req.Operation) ||
			(req.Output != 0 && page.Output == req.Output) ||
			(req.Path != "" && (page.Path == req.Path || page.Label == req.Path)) {
			selected = page.ID
			selectedRec = rec
			selectedPage = page
		}
	}
	if selectedRec == nil {
		selectedPage = pages[0]
		selectedRec = c.findRecordBySeqLocked(threadID, selectedPage.Source)
	}
	c.detailSeq++
	id := sessionpresentation.DetailID{Attachment: c.attachID, Selection: c.detailSeq}
	caller := c.threadCallers[threadID]
	if caller == "" {
		caller = "/root"
	}
	details := &sessionapi.NativeDetails{
		Source:   sessionpresentation.Thread{ID: threadID, Caller: caller},
		Selected: selected,
		Agent:    req.Agent,
		View: sessionpresentation.DetailView{
			Revision: c.detailSeq,
			ID:       id,
			Pages:    items,
		},
	}
	if selectedPage.Output > 0 {
		details.Output = c.buildDetailOutputLocked(id, selectedPage.ID, items[0].OutputVersion, selectedRec, selectedPage)
	}
	c.details = details
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) openFileDetails(req sessionapi.NativeDetailRequest) error {
	c.mu.Lock()
	workspace := c.workspace
	mainID := c.mainThread
	caller := c.threadCallers[mainID]
	if caller == "" {
		caller = "/root"
	}
	c.mu.Unlock()

	sel, ok := markdownFileTarget(workspace, req.Path)
	if !ok {
		return fmt.Errorf("invalid file path: %s", req.Path)
	}
	sel, info, err := sel.stat()
	if err != nil {
		return fmt.Errorf("open %s: %w", req.Path, err)
	}
	if info.IsDir() {
		return fmt.Errorf("%s is a directory", sel.path)
	}
	data, err := os.ReadFile(sel.path)
	if err != nil {
		return fmt.Errorf("read %s: %w", sel.path, err)
	}
	if len(data) > maxDetailFileBytes {
		data = data[:maxDetailFileBytes]
	}
	source := string(data)
	displayPath := sel.path
	if workspace != "" {
		if rel, err := filepath.Rel(workspace, sel.path); err == nil && !strings.HasPrefix(rel, "..") {
			displayPath = rel
		}
	}
	page := activity.DetailPage{
		ID:          "file:" + sel.path,
		DisplayPath: displayPath + sel.location,
		Value: activity.Value{
			Path: sel.path,
			Content: activityfeed.Content{
				Kind:  "file",
				Verb:  "File",
				Label: displayPath + sel.location,
				Code:  source,
			},
		},
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.detailSeq++
	id := sessionpresentation.DetailID{Attachment: c.attachID, Selection: c.detailSeq}
	c.details = &sessionapi.NativeDetails{
		Source:   sessionpresentation.Thread{ID: mainID, Caller: caller},
		Selected: page.ID,
		Agent:    req.Agent,
		File: &activity.DetailFile{
			Path:   sel.path,
			Source: source,
			First:  sel.first,
			Last:   sel.last,
		},
		View: sessionpresentation.DetailView{
			Revision: c.detailSeq,
			ID:       id,
			Pages:    []sessionpresentation.DetailItem{{DetailPage: page}},
		},
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) openSkillDetails(ctx context.Context, req sessionapi.NativeDetailRequest) error {
	c.mu.Lock()
	cwd := c.workspace
	mainID := c.mainThread
	caller := c.threadCallers[mainID]
	if caller == "" {
		caller = "/root"
	}
	c.mu.Unlock()

	skills := c.loadSkills(ctx, cwd)
	var b strings.Builder
	if len(skills) == 0 {
		b.WriteString("No enabled skills found for this workspace.")
	} else {
		for _, sk := range skills {
			fmt.Fprintf(&b, "- **%s**", sk.Name)
			if sk.Description != "" {
				fmt.Fprintf(&b, " — %s", sk.Description)
			}
			if sk.Path != "" {
				fmt.Fprintf(&b, " (`%s`)", sk.Path)
			}
			b.WriteByte('\n')
		}
	}
	page := activity.DetailPage{
		ID: "skills",
		Value: activity.Value{
			Content: activityfeed.Content{
				Kind: "text",
				Verb: "Skills",
				Body: strings.TrimSpace(b.String()),
			},
		},
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.detailSeq++
	id := sessionpresentation.DetailID{Attachment: c.attachID, Selection: c.detailSeq}
	c.details = &sessionapi.NativeDetails{
		Source:   sessionpresentation.Thread{ID: mainID, Caller: caller},
		Selected: page.ID,
		Agent:    req.Agent,
		View: sessionpresentation.DetailView{
			Revision: c.detailSeq,
			ID:       id,
			Pages:    []sessionpresentation.DetailItem{{DetailPage: page}},
		},
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) openErrorDetailsLocked(req sessionapi.NativeDetailRequest, overrideText string) error {
	text := overrideText
	if text == "" && req.Source != 0 {
		if rec := c.findRecordBySeqLocked(req.Displayed.ID, req.Source); rec != nil {
			text = cmp.Or(rec.entry.Text, rec.output)
		}
	}
	if text == "" {
		text = c.feedbackText
	}
	if text == "" {
		return nil
	}
	pages := activity.DetailPages(activityui.Values([]activityui.Block{{Kind: "error", Body: text}}))
	items := make([]sessionpresentation.DetailItem, 0, len(pages))
	for _, p := range pages {
		items = append(items, sessionpresentation.DetailItem{DetailPage: p})
	}
	c.detailSeq++
	id := sessionpresentation.DetailID{Attachment: c.attachID, Selection: c.detailSeq}
	caller := c.threadCallers[c.mainThread]
	if caller == "" {
		caller = "/root"
	}
	selected := ""
	if len(items) > 0 {
		selected = items[0].ID
	}
	c.details = &sessionapi.NativeDetails{
		Source:   sessionpresentation.Thread{ID: c.mainThread, Caller: caller},
		Selected: selected,
		Agent:    req.Agent,
		View: sessionpresentation.DetailView{
			Revision: c.detailSeq,
			ID:       id,
			Pages:    items,
		},
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) refreshProcesses(ctx context.Context, threadID string) {
	if c.wire == nil || threadID == "" {
		return
	}
	var resp struct {
		Data []struct {
			ItemID    string `json:"itemId"`
			ProcessID string `json:"processId"`
			Command   string `json:"command"`
			Cwd       string `json:"cwd"`
		} `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/backgroundTerminals/list", map[string]any{"threadId": threadID}, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	caller := c.threadCallers[threadID]
	if caller == "" {
		caller = "/root"
	}
	procs := make([]sessionpresentation.Process, 0, len(resp.Data))
	for _, item := range resp.Data {
		procs = append(procs, sessionpresentation.Process{
			Thread:  threadID,
			Item:    item.ItemID,
			ID:      item.ProcessID,
			Agent:   caller,
			Command: unwrapDirectShellCommand(item.Command),
		})
	}
	c.processes = procs
	c.publishLocked()
}

func (c *directNativeConnection) openProcessDetails(ctx context.Context, req sessionapi.NativeDetailRequest) error {
	c.mu.Lock()
	mainID := c.mainThread
	c.mu.Unlock()
	c.refreshProcesses(ctx, mainID)

	c.mu.Lock()
	defer c.mu.Unlock()
	caller := c.threadCallers[mainID]
	if caller == "" {
		caller = "/root"
	}
	var items []sessionpresentation.DetailItem
	for i, proc := range c.processes {
		procCopy := proc
		var seq uint64
		var outputVer uint64
		var tail []string
		outputSlot := 0
		if rec, ok := c.itemsByID[proc.Item]; ok && rec != nil {
			seq = rec.seq
			outputVer = rec.entry.Revision
			if rec.output != "" {
				tail = strings.Split(strings.TrimSuffix(rec.output, "\n"), "\n")
				outputSlot = 1
			}
		}
		if seq == 0 {
			seq = uint64(i + 1)
		}
		page := activity.DetailPage{
			ID: fmt.Sprintf("process:%s", proc.ID),
			Value: activity.Value{
				Source:  seq,
				Owner:   proc.Agent,
				Running: true,
				Output:  outputSlot,
				Tail:    tail,
				Content: activityfeed.Content{
					Kind:  "op",
					Verb:  "Run",
					Label: proc.Command,
					Code:  proc.Command,
				},
			},
		}
		items = append(items, sessionpresentation.DetailItem{
			DetailPage:    page,
			Process:       &procCopy,
			OutputVersion: outputVer,
			Retained:      len(tail) > 0,
			OutputLive:    true,
		})
	}
	if len(items) == 0 {
		items = []sessionpresentation.DetailItem{{
			DetailPage: activity.DetailPage{
				ID: "process:empty",
				Value: activity.Value{
					Content: activityfeed.Content{
						Kind: "text",
						Verb: "Background",
						Body: "No background processes running.",
					},
				},
			},
		}}
	}
	c.detailSeq++
	id := sessionpresentation.DetailID{Attachment: c.attachID, Selection: c.detailSeq}
	c.details = &sessionapi.NativeDetails{
		Source:   sessionpresentation.Thread{ID: mainID, Caller: caller},
		Selected: items[0].ID,
		Agent:    req.Agent,
		View: sessionpresentation.DetailView{
			Revision:  c.detailSeq,
			ID:        id,
			Pages:     items,
			Processes: true,
		},
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) handleStopProcess(ctx context.Context, proc sessionpresentation.Process) error {
	c.mu.Lock()
	threadID := proc.Thread
	if threadID == "" {
		threadID = c.mainThread
	}
	c.mu.Unlock()
	if threadID == "" || proc.ID == "" {
		return nil
	}
	if err := c.wire.Call(ctx, "thread/backgroundTerminals/terminate", map[string]any{
		"threadId":  threadID,
		"processId": proc.ID,
	}, nil); err != nil {
		return err
	}
	c.refreshProcesses(ctx, threadID)
	c.mu.Lock()
	wasProcessDialog := c.details != nil && c.details.View.Processes
	c.mu.Unlock()
	if wasProcessDialog {
		return c.openProcessDetails(ctx, sessionapi.NativeDetailRequest{Kind: sessionapi.NativeProcessDetails})
	}
	return nil
}

func (c *directNativeConnection) buildDetailOutputLocked(id sessionpresentation.DetailID, pageID string, version uint64, rec *directItemRecord, page activity.DetailPage) *sessionpresentation.DetailOutput {
	var lines []string
	done := page.Exited || !page.Running
	if rec != nil {
		if rec.output != "" {
			lines = strings.Split(strings.TrimSuffix(rec.output, "\n"), "\n")
		}
		done = rec.entry.OutputComplete || !rec.entry.Running
	}
	if len(lines) == 0 && len(page.Tail) > 0 {
		lines = slices.Clone(page.Tail)
	}
	return &sessionpresentation.DetailOutput{
		DetailOutputRequest: sessionpresentation.DetailOutputRequest{
			ID:      id,
			Page:    pageID,
			Version: version,
		},
		View: activityfeed.OutputView{
			Lines: lines,
			Done:  done,
		},
	}
}

func (c *directNativeConnection) handleDetailOutput(req sessionpresentation.DetailOutputRequest) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.details == nil || c.details.View.ID != req.ID {
		return
	}
	var matchedPage activity.DetailPage
	var found bool
	for _, p := range c.details.View.Pages {
		if p.ID == req.Page {
			matchedPage = p.DetailPage
			found = true
			break
		}
	}
	var rec *directItemRecord
	if found && matchedPage.Source != 0 {
		rec = c.findRecordBySeqLocked(c.details.Source.ID, matchedPage.Source)
	}
	c.details.Output = c.buildDetailOutputLocked(req.ID, req.Page, req.Version, rec, matchedPage)
	c.publishLocked()
}

func (c *directNativeConnection) openStatusPanel(ctx context.Context) error {
	var accountResp struct {
		Account *struct {
			Type  string `json:"type"`
			Email string `json:"email"`
			Plan  string `json:"planType"`
		} `json:"account"`
	}
	if c.wire != nil {
		_ = c.wire.Call(ctx, "account/read", map[string]any{"refreshToken": false}, &accountResp)
		c.refreshRateLimits(ctx)
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	var fields []sessionpresentation.StatusField
	add := func(group, label, value string) {
		if value != "" {
			fields = append(fields, sessionpresentation.StatusField{Group: group, Label: label, Value: value})
		}
	}
	add("Model", "Model", c.model)
	add("Model", "Reasoning", c.effort)
	add("Model", "Service tier", c.tier)
	add("Session", "Directory", c.workspace)
	add("Session", "Session", c.mainThread)
	if c.title != "" {
		add("Session", "Title", c.title)
	}
	if u, ok := c.threadUsage[c.mainThread]; ok {
		if u.ContextWindow > 0 {
			remaining := 100 * (1 - min(1, float64(u.ContextTokens)/float64(u.ContextWindow)))
			fields = append(fields, sessionpresentation.StatusField{
				Group:     "Usage",
				Label:     "Context window",
				Value:     fmt.Sprintf("%.0f%% left", remaining),
				Detail:    fmt.Sprintf("%s used / %s", activityui.FormatUsageTokens(u.ContextTokens), activityui.FormatUsageTokens(u.ContextWindow)),
				Remaining: &remaining,
			})
		}
		if u.InputTokens > 0 || u.OutputTokens > 0 {
			add("Usage", "Token usage", fmt.Sprintf("%s input · %s output", activityui.FormatUsageTokens(u.InputTokens), activityui.FormatUsageTokens(u.OutputTokens)))
		}
		if u.CostKnown {
			prefix := "$"
			if u.CostPartial {
				prefix = "≥$"
			}
			add("Usage", "Estimated cost", fmt.Sprintf("%s%.2f", prefix, u.EstimatedCostUSD))
		}
		if label := usagepresentation.OutputThroughputLabel(u.outputThroughput()); label != "" {
			add("Usage", "Output speed", label)
		}
	}
	for _, limit := range directCaptionLimits(c.rateLimits) {
		remaining := max(0, min(100, 100-limit.Used))
		label := limit.Label
		if label != "" {
			label = strings.ToUpper(label[:1]) + label[1:]
		}
		fields = append(fields, sessionpresentation.StatusField{
			Group:     "Account",
			Label:     label + " limit",
			Value:     fmt.Sprintf("%.0f%% left", remaining),
			Remaining: &remaining,
		})
	}
	if accountResp.Account != nil {
		switch accountResp.Account.Type {
		case "chatgpt":
			add("Account", "Sign-in", "ChatGPT")
			add("Account", "Email", accountResp.Account.Email)
			if accountResp.Account.Plan != "" && accountResp.Account.Plan != "unknown" {
				add("Account", "Plan", accountResp.Account.Plan)
			}
		case "apiKey":
			add("Account", "Sign-in", "API key")
		case "amazonBedrock":
			add("Account", "Sign-in", "Amazon Bedrock")
		}
	}
	c.panelSeq++
	c.panelKind = "status"
	c.panel = &sessionpresentation.PanelView{
		ID:        c.panelSeq,
		Fields:    fields,
		Workspace: c.workspace,
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) openSessionPanel() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	var fields []sessionpresentation.StatusField
	add := func(group, label, value string) {
		if value != "" {
			fields = append(fields, sessionpresentation.StatusField{Group: group, Label: label, Value: value})
		}
	}
	add("Session", "Thread", c.mainThread)
	add("Session", "Model", c.model)
	var totalIn, totalOut, totalRoundtrips uint64
	var totalCost float64
	costKnown, costPartial := false, false
	for _, row := range c.rosterLocked() {
		a := row.Agent
		totalIn += a.InputTokens
		totalOut += a.OutputTokens
		totalRoundtrips += a.Roundtrips
		if a.CostKnown {
			totalCost += a.Cost
			costKnown = true
			costPartial = costPartial || a.CostPartial
		}
	}
	if totalIn > 0 || totalOut > 0 {
		add("Usage", "Total tokens", fmt.Sprintf("%s input · %s output", activityui.FormatUsageTokens(totalIn), activityui.FormatUsageTokens(totalOut)))
	}
	if costKnown {
		prefix := "$"
		if costPartial {
			prefix = "≥$"
		}
		add("Usage", "Total cost", fmt.Sprintf("%s%.2f", prefix, totalCost))
	}
	if totalRoundtrips > 0 {
		add("Usage", "Roundtrips", fmt.Sprintf("%d", totalRoundtrips))
	}
	if u, ok := c.threadUsage[c.mainThread]; ok {
		if label := usagepresentation.OutputThroughputLabel(u.outputThroughput()); label != "" {
			add("Usage", "Average output", label)
		}
		if u.WorkDurationMs > 0 {
			add("Usage", "Work duration", (time.Duration(u.WorkDurationMs) * time.Millisecond).Round(100*time.Millisecond).String())
		}
	}
	c.panelSeq++
	c.panelKind = "session"
	c.panel = &sessionpresentation.PanelView{
		ID:        c.panelSeq,
		Fields:    fields,
		Workspace: c.workspace,
	}
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) handlePanelAction(ctx context.Context, action sessionpresentation.PanelAction) error {
	c.mu.Lock()
	if c.panel == nil || (action.Panel != 0 && c.panel.ID != action.Panel) {
		c.mu.Unlock()
		return nil
	}
	kind := c.panelKind
	switch action.Kind {
	case sessionpresentation.PanelClose:
		c.panel = nil
		c.panelKind = ""
		c.publishLocked()
		c.mu.Unlock()
		return nil
	case sessionpresentation.PanelMetrics:
		c.panel.MetricTab = action.Tab
		c.panel.MetricExchange = action.Exchange
		c.publishLocked()
		c.mu.Unlock()
		return nil
	case sessionpresentation.PanelRefresh:
		c.mu.Unlock()
		if kind == "session" {
			return c.openSessionPanel()
		}
		return c.openStatusPanel(ctx)
	default:
		c.mu.Unlock()
		return nil
	}
}

func (c *directNativeConnection) handleReference(ref sessionpresentation.ReferenceIntent) {
	c.mu.Lock()
	defer c.mu.Unlock()
	threadID := ref.Thread
	if threadID == "" {
		threadID = c.inputThread
	}
	if threadID == "" {
		threadID = c.mainThread
	}
	target := ref.Target
	c.effectSeq++
	c.effects = append(c.effects, sessionapi.NativeEffect{
		ID:     c.effectSeq,
		Thread: threadID,
		Target: &target,
		Selection: &sessionapi.NativeSelection{
			Description: ref.Description,
			Text:        ref.Text,
			Source:      ref.Source,
		},
	})
	c.publishLocked()
}

func (c *directNativeConnection) handleNotice(action sessionapi.NativeNoticeAction) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if action.Dismiss {
		c.feedbackText = ""
		c.feedbackAlert = false
		c.publishLocked()
		return
	}
	text := cmp.Or(action.Text, c.feedbackText)
	if text != "" {
		_ = c.openErrorDetailsLocked(sessionapi.NativeDetailRequest{Kind: sessionapi.NativeErrorDetails}, text)
	}
}

func (c *directNativeConnection) emitPresentationEffectLocked(feedback string) {
	c.effectSeq++
	c.effects = append(c.effects, sessionapi.NativeEffect{
		ID:     c.effectSeq,
		Thread: c.mainThread,
		Presentation: &sessionapi.NativePresentationEffect{
			LiveHidden:          c.liveHidden,
			InterruptLocked:     c.interruptLocked,
			PredictionsDisabled: c.predictionsDisabled,
		},
	})
	if feedback != "" {
		c.setFeedbackLocked(feedback, false)
	}
	c.publishLocked()
}

func (c *directNativeConnection) handleCopyLastResponse() {
	c.mu.Lock()
	defer c.mu.Unlock()
	threadID := c.displayedThread
	if threadID == "" {
		threadID = c.mainThread
	}
	records := c.items[threadID]
	var text string
	for i := len(records) - 1; i >= 0; i-- {
		if records[i].itemType == "agentMessage" && strings.TrimSpace(records[i].entry.Text) != "" {
			text = records[i].entry.Text
			break
		}
	}
	if text == "" {
		c.setFeedbackLocked("No assistant response available to copy", true)
		c.publishLocked()
		return
	}
	c.effectSeq++
	c.effects = append(c.effects, sessionapi.NativeEffect{
		ID:        c.effectSeq,
		Thread:    threadID,
		Clipboard: &text,
	})
	c.publishLocked()
}

func (c *directNativeConnection) forkAndSwitchThread(ctx context.Context) error {
	c.mu.Lock()
	mainID := c.mainThread
	c.mu.Unlock()
	if mainID == "" {
		return errors.New("no active thread")
	}
	var resp directThreadStartResponse
	if err := c.wire.Call(ctx, "thread/fork", map[string]any{"threadId": mainID}, &resp); err != nil {
		return fmt.Errorf("fork thread: %w", err)
	}
	c.mu.Lock()
	c.applyThreadBootstrapLocked(resp.Thread.ID, resp.Thread.Cwd, resp.Thread.Name, resp.Model, resp.ReasoningEffort, resp.ServiceTier)
	c.hydrateThreadTurnsLocked(resp.Thread.ID, resp.Thread.Turns)
	c.setFeedbackLocked("Switched to forked thread", false)
	c.publishLocked()
	c.mu.Unlock()
	c.refreshAuxiliary(ctx, resp.Thread.ID)
	return nil
}
