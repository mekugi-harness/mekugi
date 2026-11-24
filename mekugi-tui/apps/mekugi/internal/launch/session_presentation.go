package launch

import (
	"cmp"
	"context"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionpreferences/preferences"

	presentationcontract "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
)

// publishPresentation settles detached facts at a semantic transition boundary.
// Reading Model.View does not run this projection or bind an attachment.
func publishPresentation(attached *attachedPresentation, control *sessioncontrol.Client[appServerRequest], picker *composerPicker, editor **composer.Editor, settings preferences.Applied, title string, backend string, restoring bool, side *sessioncontrol.Side) sessionpresentation.View {
	m := attached.presentation
	if attached.view != "" {
		view := publishSessionView(attached, control, picker, "", settings, title, backend, restoring, side, true)
		return view
	}
	if side != nil {
		id := cmp.Or(side.Thread, "side:"+side.Key)
		m.SetSide(sessionpresentation.Thread{ID: id, Caller: sessionpresentation.SideCaller})
		if side.Thread != "" {
			attached.session.RegisterThread(observation.Thread{ID: side.Thread, Path: sessionpresentation.SideCaller})
			agent := attached.session.Activity().Agent(side.Thread)
			agent.Isolated = true
			if !agent.SettingsLive {
				agent.Settings = preferences.Evidence{Model: side.Model, Effort: side.Effort, Tier: side.Tier, TierKnown: side.Model != "" || side.Tier != ""}
			}
		}
	} else {
		m.SetSide(sessionpresentation.Thread{})
	}
	if m.DisplayingSide() {
		*editor = &side.Drafts.Editor
	} else if q := attached.conversation.Questions.Current(); q == nil || !q.IsSecret {
		*editor = &attached.conversation.Editor
	}
	view := publishSessionView(attached, control, picker, (*editor).Text, settings, title, backend, restoring, side, false)
	return view
}

// publishSessionView projects semantic owner state without choosing an editor,
// touching renderer geometry or acknowledging terminal delivery.
func publishSessionView(attached *attachedPresentation, control *sessioncontrol.Client[appServerRequest], picker *composerPicker, draft string, settings preferences.Applied, title, backend string, restoring bool, side *sessioncontrol.Side, clientEditors bool) sessionpresentation.View {
	m := attached.presentation
	// Native capability controls child input independently of displayed activity.
	m.RestrictChildInput(control.NativeInputPending)
	if side != nil {
		m.SetSide(sessionpresentation.Thread{ID: cmp.Or(side.Thread, "side:"+side.Key), Caller: sessionpresentation.SideCaller})
	} else {
		m.SetSide(sessionpresentation.Thread{})
	}
	view := m.View()
	feedbackThread := control.Thread
	if m.DisplayingSide() {
		feedbackThread = side.Thread
	}
	attached.feedback.BindThread(feedbackThread)
	var caption sessionpresentation.Caption
	if view.Displayed.ID == view.Main.ID {
		caption.Model, caption.Effort, caption.Tier = settings.Model, settings.Effort, settings.Tier
	}
	if m.DisplayingSide() {
		caption = sessionpresentation.Caption{Model: side.Model, Effort: side.Effort, Tier: side.Tier}
	}
	caption = sessionCaption(&attached.session, m, caption)
	caption.Limits = captionLimits(attached.rateLimits)
	info := sessionpresentation.MainInfo{Workspace: attached.cwd, Title: cmp.Or(control.Names.Pending(control.Thread), title), BackendVersion: backend, Skills: attached.source.ActiveSkills()["Main"]}
	if view.Main.Caller != "/root" {
		info.Caller = view.Main.Caller
	}
	if logical := attached.threadInput; logical != nil && logical.admitted {
		info.Caller = logical.caller
	}
	if approval := attached.conversation.Approvals.View(); approval.Count > 0 {
		if caller := attached.session.KnownCaller(approval.Approval.Thread); caller != "/root" {
			info.ApprovalCaller = caller
		}
	}
	picker.refreshPresetCatalog(attached, control, restoring)
	paused := false
	if thread := attached.session.FindThread(control.Thread); thread != nil {
		paused = attached.session.Activity().Lifecycle(thread).Paused
	}
	return m.Publish(sessionpresentation.MainInputs{
		Paused:        paused,
		ToolInput:     attached.session.ToolInput,
		WaitTiming:    attached.session.WaitTiming,
		Retry:         attached.session.RetryStatus(control.Thread),
		BTWRunning:    control.BTW != nil && (control.BTW.Busy || control.BTW.Starting),
		SideInterrupt: side.InterruptTarget(),
		Info:          info, ClientEditors: clientEditors,
		Menu: picker.menuView(attached.cwd, draft, &control.Live), Control: &control.Live, Feedback: &attached.feedback, Conversation: attached.conversation, Restoring: restoring,
		SideThreads: control.Requests.SideThreads(side), SideRunning: side != nil && (side.Busy || side.Starting), Processes: runningProcessCount(&attached.session),
	}, caption)
}

func attachPresentation(attached *attachedPresentation, picker *composerPicker, editor **composer.Editor, questions *nativeQuestionDock, approvals *nativeApprovalDock, main string, review **sessionreview.Review, commits **activitypresentation.CommitReader, parent context.Context, retained changes.Scope, changeSource changes.Service, activityPresent bool) {
	(*review) = attachReview(parent, (*review), retained, changeSource)
	(*commits) = attachCommitReader(&attached.session, (*commits), parent, activityPresent)
	conversation := attached.conversation
	if (*editor) != &questions.secret {
		(*editor) = &conversation.Editor
	}
	picker.State, picker.MenuState = &conversation.Completion, attached.presentation.Menu()
	questions.Questions, approvals.Approvals = &conversation.Questions, &conversation.Approvals
	m := attached.presentation
	m.Attach(sessionpresentation.Thread{ID: cmp.Or(main, "/root"), Caller: cmp.Or(attached.session.KnownCaller(main), "/root")}, false)
}

// settlePresentation runs before another intent can observe this session.
// Opening a prompt does not mark it painted or acknowledge a notice.
func settlePresentation(attached *attachedPresentation, control *sessioncontrol.Client[appServerRequest], picker *composerPicker, editor **composer.Editor, settings preferences.Applied, title string, backend string, restoring bool, side *sessioncontrol.Side, clock func() time.Time, questions *nativeQuestionDock, approvals *nativeApprovalDock, overlay bool) {
	continueRewind(attached, control, picker, restoring, clock)
	if attached.view == "" {
		autoOpenApprovals(attached, picker, editor, questions, approvals, control.Thread, overlay)
		autoOpenQuestions(attached, picker, editor, questions, approvals, control.Thread, overlay)
	}
	publishPresentation(attached, control, picker, editor, settings, title, backend, restoring, side)
	attached.session.ProjectSide(attached.childSource, attached.presentation.View().SideThread, attached.conversation.Side)
	refreshRosterRoles(&attached.session, attached.childSource)
}

// sessionCaption borrows the selected source's facts; it retains no accounting state.
func sessionCaption(session *sessionpresentation.Attachment, m *sessionpresentation.Model, caption sessionpresentation.Caption) sessionpresentation.Caption {
	view := m.View()
	if agent := session.Agent(view.Displayed.Caller); agent != nil {
		if view.Displayed.ID != view.Main.ID {
			caption.Model, caption.Effort = agent.Settings.Model, agent.Settings.Effort
			if agent.Settings.TierKnown {
				caption.Tier = cmp.Or(agent.Settings.Tier, "default")
			}
		}
		caption.ContextKnown, caption.ContextTokens, caption.ContextWindow = agent.ContextKnown, agent.ContextTokens, agent.ContextWindow
		caption.Items = m.StatusItems(presentationcontract.Thread{ID: view.Displayed.ID, OutputThroughput: agent.OutputThroughput}, presentationcontract.ComposerCaption)
	}
	return caption
}
