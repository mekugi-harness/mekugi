package launch

import (
	"cmp"
	"context"
	json "encoding/json/v2"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/rollout"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/shellinstrumentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"
)

func restoreThreadSettings(session *sessionpresentation.Attachment, info binding.ThreadInfo) {
	s := session
	agent := s.Activity().Agent(info.ID)
	applyContextUsage(agent, info)
	if !agent.SettingsLive {
		settings := rollout.ResumeSettings(info.ID, info.Path, time.Time{})
		if model, ok := settings["model"].(string); ok {
			agent.Settings.Model = model
			agent.Settings.Effort, _ = settings["model_reasoning_effort"].(string)
			if tier, known := settings["service_tier"]; known {
				agent.Settings.Tier, _ = tier.(string)
				agent.Settings.TierKnown = true
			}
		}
	}
}

// sessionEvent owns live enrichment and effects around shared presentation.
func (session *nativeSession) sessionEvent(attached *attachedPresentation, clock func() time.Time, method string, p binding.Notification) (bool, error) {
	s := &attached.session
	if s.KnownCaller(session.Client.Thread) == "" {
		return false, nil
	}
	if p.ThreadID == session.Client.Thread && p.TurnID != "" && p.Item.Type == "userMessage" && (method == "item/started" || method == "item/completed") {
		session.Client.HasUserTurn = true
	}
	if p.ThreadID != "" && p.ThreadID != session.Client.Thread {
		s.Caller(p.ThreadID)
		agent := s.Activity().Agent(p.ThreadID)
		if session.Client.Requests.WorkSide(p.ThreadID) {
			agent.Isolated, agent.AudiencePending = true, false
		} else if side := attached.conversation.Side; side != nil && side.OneOff && agent.ParentThread == "" {
			agent.AudiencePending = true
		}
	}
	if method == "hook/started" || method == "hook/completed" {
		return true, hookEvent(attached, &session.Client, session.activity, session.review, &session.approvals, &session.pendingEditReceipts, session.retainedSession, clock, method, p)
	}
	if method == "item/started" {
		observeShellItem(&session.Client.Shell, session.Client.Thread, p.ThreadID, p.Item)
	}
	if command := p.Command; command != nil {
		key := command.Key
		if method == "item/completed" {
			session.execTrack.Completed(key)
		}
		switch {
		case method == "item/started":
			if script, ok := shellinstrumentation.CommandScript(command.Command); ok {
				session.execTrack.Start(key, script)
			}
		case method == "item/completed" && (attached.session.Commands()[key] == nil || !session.execTrack.Tracking(key)):
			session.execTrack.Forget(key)
		}
	}

	key := [3]string{p.ThreadID, p.TurnID, cmp.Or(p.ItemID, p.Item.ID)}
	event := p.Observation(method, presentationTime(clock))
	// This invocation configures native token-budget reset. Historical events
	// carry their retained method through restoration instead of this live path.
	if event.Item.Type == "contextCompaction" && event.Item.CompactionMethod == "" {
		event.Item.CompactionMethod = observation.ContextReset
	}

	facts := sessionpresentation.ObservationInput{Event: event, Text: binding.RetainedTranscript(event), Main: session.Client.Thread, Selected: session.Client.Turn, Start: session.Client.InterruptTarget().Start, Workspace: attached.cwd, Tracked: session.execTrack.Tracking(key)}
	facts.AuxiliaryTurn = p.ThreadID == session.Client.Thread && session.Client.AuxiliaryTurn(p.Turn.ID)
	if p.Item.Type == "commandExecution" && session.nativeTrace != nil {
		if ms, known := session.nativeTrace.CommandTimeout(p.ThreadID, key[2]); known {
			facts.CommandTimeout = strconv.FormatUint(ms/1000, 10)
			if fraction := ms % 1000; fraction != 0 {
				facts.CommandTimeout += "." + strings.TrimRight(fmt.Sprintf("%03d", fraction), "0")
			}
			facts.CommandTimeout += "s"
		}
	}
	if method == "turn/completed" {
		endTurnPreviews(session.execWindows, session.review, attached.cwd, p.ThreadID, p.Turn.ID)
	}
	if method == "rawResponse/completed" || method == "thread/tokenUsage/updated" || method == "turn/completed" {
		if method == "thread/tokenUsage/updated" && p.ThreadID == session.Client.Thread {
			session.exitUsage = p.TokenUsage.Total
		}
		s.Caller(p.ThreadID)
		observeCost(session.usage, p.ThreadID, s.Activity().Agent(p.ThreadID))
	}
	result := sessionpresentation.ReduceObservation(s, attached.presentation, attached.source, attached.childSource, attached.transcript, session.activity.Outputs(s.Scope()), facts)
	if result.Compaction != nil {
		if err := retainCompaction(session.retainedSession, *result.Compaction); err != nil {
			setNotice(&attached.feedback, presentationTime(clock), "Compaction measurements could not be retained: "+err.Error(), true)
		}
	}
	if result.Started && session.review != nil {
		session.review.Preview().SetTurn(p.ThreadID, p.Turn.ID, true)
	}
	if method == "thread/started" && p.Thread.ID != "" && p.Thread.ID != session.Client.Thread {
		restoreThreadSettings(&attached.session, p.Thread)
	}
	if result.Publish {
		attached.hooks.deliverActivity(attached.executionPresentation, result.Deliveries)
		applyActivity(attached, &session.approvals, session.review, &session.pendingEditReceipts, session.Client.Thread, clock, nil, nil)
	}
	return result.Handled, nil
}

// flushStreamOutput admits changed segment facts and publishes output deltas.
// Reveal-only updates never replace neutral command records. Late output cannot
// reopen a completed invocation.
func (session *nativeSession) flushStreamOutput(attached *attachedPresentation, clock func() time.Time, parent context.Context) {
	s := &attached.session
	var entries []sessionpresentation.ActivityEntry
	for key, run := range attached.session.Commands() {
		tracked := flushTrackedCommand(attached, session.execTrack, clock, parent, session.retained, session.segments, &session.commandSegmentWrites, &session.commandSegmentPending, session.issues, key, run)
		entries = append(entries, tracked...)
	}

	if len(entries) == 0 {
		return
	}
	slices.SortFunc(entries, func(a, b sessionpresentation.ActivityEntry) int { return cmp.Compare(a.Seq, b.Seq) })
	for i := range entries {
		entries[i].Seq = s.Activity().NextSequence()
	}
	applyActivity(attached, &session.approvals, session.review, &session.pendingEditReceipts, session.Client.Thread, clock, entries, nil)
}

// observeResponse uses only fields present in the native completion. Native
// normalized counters default absent categories to zero; verify the corresponding
// upstream usage fields before pricing them.
func (session *nativeSession) observeResponse(params []byte) error {
	var event binding.ResponseCompleted
	if err := json.Unmarshal(params, &event); err != nil {
		return err
	}
	if event.ThreadID == "" || event.ResponseID == "" {
		return nil
	}
	var counts inference.Usage
	if u := event.Usage; u != nil {
		counts = inference.Usage{TotalsKnown: true, InputTokens: u.InputTokens,
			UncachedInputTokens: u.InputTokens - min(u.CachedInputTokens, u.InputTokens),
			CacheWriteTokens:    u.CacheWriteInputTokens, OutputTokens: u.OutputTokens,
			ReasoningTokens: u.ReasoningOutputTokens,
			Incomplete: event.UsageMetadata == nil || event.UsageMetadata.Metadata.Input.Cached == nil ||
				event.UsageMetadata.Metadata.Output.Reasoning == nil,
			Inconsistent: u.CachedInputTokens > u.InputTokens || u.ReasoningOutputTokens > u.OutputTokens,
		}
	}
	duration := time.Duration(event.RequestDurationMS) * time.Millisecond
	session.usage.ObserveResponse(event.ThreadID, event.Model, event.ServiceTier, event.OpenAIEndpoint, counts, duration)
	capture := session.sessionCapture
	if capture != nil {
		capture.ObserveResponse(event.ThreadID, event.TurnID, event.Model, counts, duration)
	}
	return nil
}

// observeCost reads all consumption metrics from the same canonical owner.
// App-server context and exit usage are separate, never priced or added here.
func observeCost(usage *accounting.Service, thread string, agent *sessionpresentation.ActivityAgent) {
	if usage == nil {
		return
	}
	report := usage.Snapshot(thread)
	agent.Roundtrips = report.Roundtrips
	agent.OutputThroughput = report.OutputThroughput
	agent.InputTokens, agent.OutputTokens = report.InputTokens, report.OutputTokens
	agent.TokensKnown = report.TotalsKnown
	agent.Cost = report.Cost
	agent.CostKnown = agent.TokensKnown && report.CostKnown
	agent.CostPartial = report.MissingUsage != 0 || report.PriorUnknown || report.Incomplete
	agent.UsagePartial = agent.CostPartial
	agent.RoundtripsPartial = report.PriorUnknown
}

func restoreUsage(usage *accounting.Service, info binding.ThreadInfo) {
	if len(info.Turns) > 0 {
		usage.Restore(info.ID, true)
	}
}
