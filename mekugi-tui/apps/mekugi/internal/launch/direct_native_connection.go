package launch

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"maps"
	"math"
	"os"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/presentation"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/completion"
	conversation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
	usagepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/presentation"
)

// nativeSessionConnection abstracts the frame stream and semantic operation
// transport consumed by nativeClient.
type nativeSessionConnection interface {
	Next() (sessionapi.NativeEvent, error)
	Send(ctx context.Context, target sessionapi.NativeTarget, operation sessionapi.NativeOperation) (sessionapi.NativeResult, error)
	Attach(ctx context.Context, thread string) (nativeSessionConnection, error)
	Close() error
}

type directPendingServerRequest struct {
	id         jsontext.Value
	method     string
	threadID   string
	turnID     string
	itemID     string
	rawParams  jsontext.Value
	approval   *prompt.ApprovalView
	question   *prompt.CallView
	secret     bool
	shellGuard bool
}

type directItemRecord struct {
	id       string
	threadID string
	turnID   string
	itemType string
	seq      uint64
	entry    activity.Entry
	output   string
}

type directThreadUsage struct {
	TotalTokens           uint64
	InputTokens           uint64
	OutputTokens          uint64
	CachedInputTokens     uint64
	CacheWriteInputTokens uint64
	ReasoningOutputTokens uint64
	ContextTokens         uint64
	ContextWindow         uint64
	EstimatedCostUSD      float64
	CostKnown             bool
	CostPartial           bool
	OutputTokensPerSecond float64
	HasThroughput         bool
	WorkDurationMs        int64
	Roundtrips            uint64
}

func (u directThreadUsage) outputThroughput() inference.OutputThroughput {
	if !u.HasThroughput {
		return inference.OutputThroughput{}
	}
	measured := u.Roundtrips
	if measured == 0 {
		measured = 1
	}
	return inference.OutputThroughput{
		OutputTokens:     uint64(math.Round(u.OutputTokensPerSecond * 1000)),
		DurationNanos:    uint64(1000 * time.Second),
		MeasuredRequests: measured,
	}
}

type directNativeOptions struct {
	socket                string
	workspace             string
	routerArgs            []string
	args                  []string
	resume                string
	profile               string
	requireCustomProvider bool
	yolo                  bool
	configArgs            []string
}

var directAttachmentSeq atomic.Uint64

// directNativeConnection connects the Go TUI directly to mekugi-app-server's
// Unix-socket WebSocket listener. Native server owners drive transcript,
// roster, command output, saved changes, and prompts; Go retains no session
// state machine or control loop.
type directNativeConnection struct {
	opts       directNativeOptions
	wire       *nativeAppServerWire
	attachment string
	attachID   uint64

	ctx       context.Context
	cancel    context.CancelFunc
	closeOnce sync.Once

	events chan sessionapi.NativeEvent
	errCh  chan error

	mu              sync.Mutex
	closed          bool
	revision        uint64
	seq             uint64
	mainThread      string
	displayedThread string
	inputThread     string
	sideThread      string
	btwThread       string
	btwMainThread   string
	btwExchangeID   uint64
	btwQuestion     string
	btwAnswer       strings.Builder
	btwRunning      bool
	workspace       string
	title           string
	model           string
	effort          string
	tier            string
	busy            bool
	activeTurn      string
	sideActiveTurn  string
	archived        bool
	usage           sessionapi.NativeUsage
	contextTokens   uint64
	contextWindow   uint64
	threadUsage     map[string]directThreadUsage
	rateLimits      [2]*sessionpresentation.CaptionLimit
	prediction      string
	predictionTurn  string
	feedbackText    string
	feedbackAlert   bool
	feedbackRev     uint64

	items       map[string][]*directItemRecord
	itemsByID   map[string]*directItemRecord
	mainOutput  []activity.OutputResult
	agentOutput []activity.OutputResult

	family           *sessionapi.Family
	threadCallers    map[string]string
	review           *changes.ReviewView
	reviewRev        uint64
	queue            []conversation.PendingText
	resumeView       *sessionpresentation.ResumeView
	resumeCursor     string
	requests         map[string]*directPendingServerRequest
	requestOrder     []string
	details          *sessionapi.NativeDetails
	preferences      jsontext.Value
	focused          bool
	menu             *sessionpresentation.MenuView
	menuSeq          uint64
	completionTarget conversation.EditorTarget
	dismissedComp    completion.Target
	skillsCwd        string
	skillsLoaded     bool
	skills           []completion.Choice
	skillsGen        uint64
	effectSeq        uint64
	effects          []sessionapi.NativeEffect
}

func openDirectNativeConnection(ctx context.Context, socket, workspace string, routerArgs, args []string, resume, profile string) (*directNativeConnection, error) {
	parsedRouter, err := launchoptions.Parse(routerArgs)
	if err != nil {
		return nil, err
	}
	configArgs, _, yolo, err := appServerArgs(expandReasoningShortcuts(args))
	if err != nil {
		return nil, err
	}
	opts := directNativeOptions{
		socket:                socket,
		workspace:             workspace,
		routerArgs:            slices.Clone(routerArgs),
		args:                  slices.Clone(args),
		resume:                resume,
		profile:               profile,
		requireCustomProvider: parsedRouter.RequireCustomProvider,
		yolo:                  yolo,
		configArgs:            nativeAppServerArgs(configArgs),
	}
	return newDirectNativeConnection(ctx, opts, "")
}

func newDirectNativeConnection(ctx context.Context, opts directNativeOptions, attachThread string) (*directNativeConnection, error) {
	wire, err := dialNativeAppServer(ctx, opts.socket)
	if err != nil {
		return nil, err
	}
	connCtx, cancel := context.WithCancel(context.Background())
	attachID := directAttachmentSeq.Add(1)
	c := &directNativeConnection{
		opts:          opts,
		wire:          wire,
		attachment:    fmt.Sprintf("direct-%d", attachID),
		attachID:      attachID,
		ctx:           connCtx,
		cancel:        cancel,
		events:        make(chan sessionapi.NativeEvent, 64),
		errCh:         make(chan error, 1),
		workspace:     opts.workspace,
		items:         make(map[string][]*directItemRecord),
		itemsByID:     make(map[string]*directItemRecord),
		threadCallers: make(map[string]string),
		threadUsage:   make(map[string]directThreadUsage),
		requests:      make(map[string]*directPendingServerRequest),
		focused:       true,
	}

	if err := c.bootstrap(ctx, attachThread); err != nil {
		_ = c.Close()
		return nil, err
	}
	c.mu.Lock()
	c.publishLocked()
	c.mu.Unlock()
	go c.eventLoop()
	return c, nil
}

func (c *directNativeConnection) bootstrap(ctx context.Context, attachThread string) error {
	targetResume := attachThread
	if targetResume == "" {
		targetResume = c.opts.resume
	}
	switch targetResume {
	case "--pick":
		return c.loadResumeList(ctx, "", false)
	case "--last":
		listParams := map[string]any{"limit": 1}
		if c.opts.workspace != "" {
			listParams["cwd"] = c.opts.workspace
		}
		var listed struct {
			Data []struct {
				ID string `json:"id"`
			} `json:"data"`
		}
		if err := c.wire.Call(ctx, "thread/list", listParams, &listed); err != nil {
			return fmt.Errorf("list threads for resume --last: %w", err)
		}
		if len(listed.Data) == 0 {
			return errors.New("no saved threads available to resume")
		}
		targetResume = listed.Data[0].ID
	}

	if targetResume != "" {
		return c.resumeThread(ctx, targetResume)
	}
	return c.startFreshThread(ctx)
}

func (c *directNativeConnection) familyParams() map[string]any {
	if c.opts.profile == "" && !c.opts.requireCustomProvider {
		return nil
	}
	env := make(map[string]string)
	for _, entry := range serverEnvironment(os.Environ()) {
		if k, v, ok := strings.Cut(entry, "="); ok && k != "" {
			env[k] = v
		}
	}
	family := map[string]any{
		"environment": env,
	}
	if c.opts.profile != "" {
		family["profile"] = c.opts.profile
	}
	if c.opts.requireCustomProvider {
		family["requireCustomProvider"] = true
	}
	return family
}

func (c *directNativeConnection) startFreshThread(ctx context.Context) error {
	params := map[string]any{
		"cwd": c.opts.workspace,
	}
	if cfg := parseDirectConfigOverrides(c.opts.configArgs); len(cfg) > 0 {
		params["config"] = cfg
	}
	if c.opts.yolo {
		params["approvalPolicy"] = "never"
		params["sandbox"] = "danger-full-access"
	}
	if family := c.familyParams(); family != nil {
		params["family"] = family
	}
	var resp directThreadStartResponse
	if err := c.wire.Call(ctx, "thread/start", params, &resp); err != nil {
		return fmt.Errorf("start native thread: %w", err)
	}
	c.mu.Lock()
	c.applyThreadBootstrapLocked(resp.Thread.ID, resp.Thread.Cwd, resp.Thread.Name, resp.Model, resp.ReasoningEffort, resp.ServiceTier)
	c.mu.Unlock()
	c.refreshAuxiliary(ctx, resp.Thread.ID)
	return nil
}

func (c *directNativeConnection) resumeThread(ctx context.Context, threadID string) error {
	params := map[string]any{
		"threadId": threadID,
	}
	if c.opts.yolo {
		params["approvalPolicy"] = "never"
		params["sandbox"] = "danger-full-access"
	}
	if family := c.familyParams(); family != nil {
		params["family"] = family
	}
	var resp directThreadStartResponse
	if err := c.wire.Call(ctx, "thread/resume", params, &resp); err != nil {
		return fmt.Errorf("resume native thread %s: %w", threadID, err)
	}
	c.mu.Lock()
	c.applyThreadBootstrapLocked(resp.Thread.ID, resp.Thread.Cwd, resp.Thread.Name, resp.Model, resp.ReasoningEffort, resp.ServiceTier)
	c.hydrateThreadTurnsLocked(resp.Thread.ID, resp.Thread.Turns)
	c.mu.Unlock()
	c.refreshAuxiliary(ctx, resp.Thread.ID)
	return nil
}

func (c *directNativeConnection) applyThreadBootstrapLocked(threadID, cwd, name, model, effort, tier string) {
	if c.mainThread != threadID {
		c.activeTurn = ""
	}
	c.mainThread = threadID
	c.displayedThread = threadID
	c.inputThread = threadID
	if cwd != "" {
		c.workspace = cwd
	}
	if name != "" {
		c.title = name
	}
	if model != "" {
		c.model = model
	}
	if effort != "" {
		c.effort = effort
	}
	if tier != "" {
		c.tier = tier
	}
	c.threadCallers[threadID] = "/root"
	c.resumeView = nil
}

func (c *directNativeConnection) refreshAuxiliary(ctx context.Context, threadID string) {
	if threadID == "" {
		return
	}
	c.refreshSubtree(ctx, threadID)
	c.refreshChanges(ctx, threadID)
	c.refreshQueue(ctx, threadID)
	c.refreshRateLimits(ctx)
}

func (c *directNativeConnection) Next() (sessionapi.NativeEvent, error) {
	select {
	case ev, ok := <-c.events:
		if !ok {
			select {
			case err := <-c.errCh:
				return sessionapi.NativeEvent{}, err
			default:
				return sessionapi.NativeEvent{}, io.EOF
			}
		}
		return ev, nil
	case err := <-c.errCh:
		return sessionapi.NativeEvent{}, err
	}
}

func (c *directNativeConnection) Attach(ctx context.Context, thread string) (nativeSessionConnection, error) {
	return newDirectNativeConnection(ctx, c.opts, thread)
}

func (c *directNativeConnection) Close() error {
	var err error
	c.closeOnce.Do(func() {
		c.cancel()
		err = c.wire.Close()
	})
	return err
}

func (c *directNativeConnection) finish(err error) {
	select {
	case c.errCh <- err:
	default:
	}
	_ = c.Close()
}

func (c *directNativeConnection) eventLoop() {
	defer func() {
		c.mu.Lock()
		c.closed = true
		close(c.events)
		c.mu.Unlock()
	}()
	for {
		select {
		case <-c.ctx.Done():
			return
		case msg, ok := <-c.wire.Events():
			if !ok {
				select {
				case c.errCh <- io.EOF:
				default:
				}
				return
			}
			c.handleServerMessage(msg)
		}
	}
}

func (c *directNativeConnection) publishLocked() {
	if c.closed {
		return
	}
	c.revision++
	ev := c.buildEventLocked()
	select {
	case c.events <- ev:
	default:
		select {
		case <-c.events:
		default:
		}
		select {
		case c.events <- ev:
		default:
		}
	}
}

func (c *directNativeConnection) buildEventLocked() sessionapi.NativeEvent {
	displayed := c.displayedThread
	if displayed == "" {
		displayed = c.mainThread
	}
	input := c.inputThread
	if input == "" {
		input = c.mainThread
	}
	mainCaller := c.threadCallers[c.mainThread]
	if mainCaller == "" {
		mainCaller = "/root"
	}
	displayedCaller := c.threadCallers[displayed]
	if displayedCaller == "" {
		displayedCaller = mainCaller
	}
	inputCaller := c.threadCallers[input]
	if inputCaller == "" {
		inputCaller = mainCaller
	}
	var sideThread sessionpresentation.Thread
	if c.sideThread != "" {
		sideThread = sessionpresentation.Thread{ID: c.sideThread, Caller: "side"}
	}

	displayedUsage, hasDisplayedUsage := c.threadUsage[displayed]
	if !hasDisplayedUsage {
		displayedUsage = c.threadUsage[c.mainThread]
	}
	captionCtxTokens := c.contextTokens
	captionCtxWindow := c.contextWindow
	if displayedUsage.ContextWindow > 0 || displayedUsage.ContextTokens > 0 {
		captionCtxTokens = displayedUsage.ContextTokens
		captionCtxWindow = displayedUsage.ContextWindow
	}
	var captionItems []presentation.Item
	if label := usagepresentation.OutputThroughputLabel(displayedUsage.outputThroughput()); label != "" {
		captionItems = append(captionItems, presentation.Item{
			ID:   usagepresentation.Throughput.ID,
			Text: label,
		})
	}

	view := sessionpresentation.View{
		DisplayedWorkspace: c.workspace,
		Info: sessionpresentation.MainInfo{
			Workspace:      c.workspace,
			Title:          c.title,
			BackendVersion: c.wire.UserAgent(),
			Caller:         mainCaller,
		},
		Resume:    c.resumeView,
		Menu:      cloneMenuView(c.menu),
		Version:   sessionpresentation.Version{Attachment: c.attachID, Revision: c.revision},
		Main:      sessionpresentation.Thread{ID: c.mainThread, Caller: mainCaller},
		Displayed: sessionpresentation.Thread{ID: displayed, Caller: displayedCaller},
		Input:     sessionpresentation.Thread{ID: input, Caller: inputCaller},
		Status: sessionpresentation.MainStatus{
			State: sessionpresentation.Label{
				Text:    c.statusTextLocked(),
				Running: c.busy,
			},
			Animating: c.busy,
		},
		Caption: sessionpresentation.Caption{
			Model:         c.model,
			Effort:        c.effort,
			Tier:          c.tier,
			ContextTokens: captionCtxTokens,
			ContextWindow: captionCtxWindow,
			ContextKnown:  captionCtxWindow > 0,
			Items:         captionItems,
			Limits:        directCaptionLimits(c.rateLimits),
		},
		Pending: conversation.PendingView{
			CanSteer: c.busy,
			Queued:   slices.Clone(c.queue),
		},
		Draft: conversation.DraftView{
			Available:  c.mainThread != "" && c.resumeView == nil,
			Prediction: c.prediction,
		},
		Interrupt:          sessioncontrol.InterruptTarget{Thread: input, Turn: c.activeTurnForLocked(input)},
		InterruptAvailable: c.busy || c.btwRunning,
		SideThread:         sideThread,
		BTWRunning:         c.btwRunning,
	}
	if c.displayedThread != "" && c.displayedThread != c.mainThread && c.displayedThread != c.sideThread {
		view.ReadOnly = sessionpresentation.ChildDisplayed
	}
	if c.feedbackText != "" {
		view.Status.SetFeedback(sessionpresentation.Label{Text: c.feedbackText, Alert: c.feedbackAlert}, c.feedbackRev)
	}
	if c.sideThread != "" {
		view.Side = &sessionpresentation.SideToken{ID: 1, Key: c.sideThread, Label: "Side"}
	}
	if c.btwThread != "" || c.btwQuestion != "" {
		var answers []conversation.Answer
		if text := c.btwAnswer.String(); text != "" {
			answers = []conversation.Answer{{Text: text}}
		}
		status := "Done"
		if c.btwRunning {
			status = "Running"
		}
		view.BTW = &conversation.SideView{
			Key:      c.btwThread,
			ID:       c.btwExchangeID,
			Question: c.btwQuestion,
			Status:   status,
			Answer:   answers,
		}
	}

	var approvals []prompt.ApprovalView
	var questions []prompt.CallView
	for _, id := range c.requestOrder {
		req := c.requests[id]
		if req == nil {
			continue
		}
		if req.approval != nil {
			approvals = append(approvals, *req.approval)
		}
		if req.question != nil {
			questions = append(questions, *req.question)
		}
	}

	mainPub := c.publicationForThreadLocked(c.mainThread)
	agentsPub := mainPub
	if displayed != c.mainThread {
		agentsPub = c.publicationForThreadLocked(displayed)
	}
	var family *sessionapi.Family
	if c.family != nil {
		cp := *c.family
		cp.Agents = &activity.Publication{Roster: c.rosterLocked()}
		cp.Status = sessionpresentation.Label{Text: c.statusTextLocked(), Running: c.busy}
		family = &cp
	}

	frame := sessionapi.NativeFrame{
		Questions:        questions,
		Approvals:        approvals,
		View:             view,
		Main:             mainPub,
		Agents:           agentsPub,
		Family:           family,
		Review:           c.review,
		Details:          c.details,
		MainOutput:       slices.Clone(c.mainOutput),
		AgentOutput:      slices.Clone(c.agentOutput),
		Effects:          slices.Clone(c.effects),
		CompletionTarget: c.completionTarget,
		Facts: sessionapi.NativeSessionFacts{
			Preferences:      c.preferences,
			Usage:            c.usage,
			Archived:         c.archived,
			PredictionThread: c.mainThread,
			PredictionTurn:   c.predictionTurn,
			ActionRequired:   len(approvals) > 0 || len(questions) > 0,
			Busy:             c.busy,
		},
	}
	return sessionapi.NativeEvent{
		Attachment: c.attachment,
		Frame:      frame,
	}
}

func (c *directNativeConnection) statusTextLocked() string {
	if c.busy {
		return "Working"
	}
	if c.mainThread == "" {
		return "Connecting…"
	}
	return "Ready"
}

func (c *directNativeConnection) activeTurnForLocked(threadID string) string {
	if threadID != "" && threadID == c.sideThread {
		return c.sideActiveTurn
	}
	return c.activeTurn
}

func (c *directNativeConnection) publicationForThreadLocked(threadID string) *activity.Publication {
	records := c.items[threadID]
	entries := make([]activity.Entry, 0, len(records))
	var lastSeq uint64
	for _, rec := range records {
		entry := rec.entry
		entry.Spans = slices.Clone(entry.Spans)
		entry.Images = slices.Clone(entry.Images)
		entry.Outputs = maps.Clone(entry.Outputs)
		entry.Values = slices.Clone(entry.Values)
		entries = append(entries, entry)
		if entry.Seq > lastSeq {
			lastSeq = entry.Seq
		}
	}
	pub := &activity.Publication{
		Entries: entries,
		LastSeq: lastSeq,
		Roster:  c.rosterLocked(),
	}
	return pub
}

func (c *directNativeConnection) rosterLocked() []activity.AgentRow {
	var rows []activity.AgentRow
	if c.family != nil && c.family.Agents != nil && len(c.family.Agents.Roster) > 0 {
		rows = slices.Clone(c.family.Agents.Roster)
	} else if c.mainThread != "" {
		caller := c.threadCallers[c.mainThread]
		if caller == "" {
			caller = "/root"
		}
		rows = []activity.AgentRow{{
			Agent: activity.Agent{
				AgentFacts: activity.AgentFacts{
					Name:   caller,
					Thread: c.mainThread,
				},
			},
		}}
	}
	for i := range rows {
		agent := &rows[i].Agent
		if agent.Thread == c.mainThread {
			agent.Responding = c.busy
		}
		if u, ok := c.threadUsage[agent.Thread]; ok {
			agent.InputTokens = u.InputTokens
			agent.OutputTokens = u.OutputTokens
			agent.TokensKnown = u.TotalTokens > 0 || u.InputTokens > 0 || u.OutputTokens > 0
			agent.ContextTokens = u.ContextTokens
			agent.ContextWindow = u.ContextWindow
			agent.ContextKnown = u.ContextWindow > 0
			agent.Cost = u.EstimatedCostUSD
			agent.CostKnown = u.CostKnown
			agent.CostPartial = u.CostPartial
			agent.Roundtrips = u.Roundtrips
			if u.WorkDurationMs > 0 {
				agent.WorkElapsed = time.Duration(u.WorkDurationMs) * time.Millisecond
				agent.WorkKnown = true
			}
			agent.OutputThroughput = u.outputThroughput()
			if label := usagepresentation.OutputThroughputLabel(agent.OutputThroughput); label != "" {
				agent.StatusItems = []presentation.Item{{
					ID:   usagepresentation.Throughput.ID,
					Text: label,
				}}
			}
		}
	}
	return rows
}

func parseDirectConfigOverrides(args []string) map[string]any {
	cfg := make(map[string]any)
	for i := 0; i < len(args); i++ {
		if (args[i] == "-c" || args[i] == "--config") && i+1 < len(args) {
			i++
			k, v, ok := strings.Cut(args[i], "=")
			if !ok || k == "" {
				continue
			}
			v = strings.TrimSpace(v)
			switch {
			case v == "true":
				cfg[k] = true
			case v == "false":
				cfg[k] = false
			case len(v) >= 2 && v[0] == '"' && v[len(v)-1] == '"':
				cfg[k] = v[1 : len(v)-1]
			default:
				cfg[k] = v
			}
		}
	}
	return cfg
}

type directThreadStartResponse struct {
	Thread          directWireThread `json:"thread"`
	Model           string           `json:"model"`
	ReasoningEffort string           `json:"reasoningEffort"`
	ServiceTier     string           `json:"serviceTier"`
}

type directWireThread struct {
	ID        string           `json:"id"`
	Name      string           `json:"name"`
	Preview   string           `json:"preview"`
	Cwd       string           `json:"cwd"`
	UpdatedAt int64            `json:"updatedAt"`
	AgentPath string           `json:"agentPath"`
	AgentRole string           `json:"agentRole"`
	Status    jsontext.Value   `json:"status"`
	Turns     []directWireTurn `json:"turns"`
}

type directWireTurn struct {
	ID     string           `json:"id"`
	Status string           `json:"status"`
	Items  []jsontext.Value `json:"items"`
}

type directThreadChangesResponse struct {
	ThreadID       string `json:"threadId"`
	Workspace      string `json:"workspace"`
	TotalAdditions int    `json:"totalAdditions"`
	TotalDeletions int    `json:"totalDeletions"`
	Changes        []struct {
		ChangeID   string   `json:"changeId"`
		Sequence   uint64   `json:"sequence"`
		ThreadID   string   `json:"threadId"`
		SourceKind string   `json:"sourceKind"`
		CreatedAt  int64    `json:"createdAt"`
		Paths      []string `json:"paths"`
		Incomplete bool     `json:"incomplete"`
	} `json:"changes"`
	Files []struct {
		Path          string   `json:"path"`
		MovedFrom     string   `json:"movedFrom"`
		Status        string   `json:"status"`
		UnifiedDiff   string   `json:"unifiedDiff"`
		Additions     int      `json:"additions"`
		Deletions     int      `json:"deletions"`
		IsBinary      bool     `json:"isBinary"`
		CoverageNotes []string `json:"coverageNotes"`
	} `json:"files"`
}
