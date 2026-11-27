package launch

import (
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	activityui "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/terminalview"
)

type transcriptDetailNav struct {
	view          *activityui.View
	thread        string
	active        activityui.DetailTarget
	savedFollow   bool
	savedOffset   int
	hasSavedPos   bool
	pendingLast   bool
	pendingDetail bool
	lastDetailID  sessionpresentation.DetailID
}

func (c *nativeClient) buildTranscriptDetailRequest(view *activityui.View, thread string, target activityui.DetailTarget, last bool) sessionapi.NativeDetailRequest {
	u := c.renderer.terminal
	request := sessionapi.NativeDetailRequest{
		Kind:      sessionapi.NativeEntryDetails,
		Source:    target.Entry,
		Agent:     view == u.Agents,
		Displayed: sessionpresentation.Thread{ID: thread},
	}
	if target.Entry == 0 {
		snippet, detail := target.Snippet, target.Detail
		if last && target.LastSnippet != (activityui.Snippet{}) {
			snippet, detail = target.LastSnippet, target.LastDetail
		}
		request.Source, request.Path = detail.Source, snippet.Path
		request.Edit = snippet.Block == activityui.EditNavigationSnippet
		request.Operation, request.Output, request.Sources = detail.Operation, detail.Output, detail.Sources
	}
	return request
}

func (c *nativeClient) openTranscriptDetail(version sessionpresentation.Version, view *activityui.View, thread string, target activityui.DetailTarget, selectLast bool) {
	if view == nil || target.Zero() {
		return
	}
	prev := c.detailNav
	if !c.detailNav.hasSavedPos || c.detailNav.view != view {
		if c.detailNav.view != nil && c.detailNav.view != view {
			c.detailNav.view.ClearActiveDetail()
		}
		c.detailNav.savedFollow = view.Following
		c.detailNav.savedOffset = view.Offset
		c.detailNav.hasSavedPos = true
	}
	active := target
	if selectLast && target.LastSnippet != (activityui.Snippet{}) {
		active.Snippet = target.LastSnippet
		active.Detail = target.LastDetail
	}
	active = view.ResolveDetailTarget(active)
	c.detailNav.view = view
	c.detailNav.thread = thread
	c.detailNav.active = active
	c.detailNav.pendingLast = selectLast
	c.detailNav.pendingDetail = true
	view.SetActiveDetail(active, true)
	request := c.buildTranscriptDetailRequest(view, thread, target, selectLast)
	before := len(c.calls)
	c.details(version, request)
	if len(c.calls) > before {
		call := c.calls[len(c.calls)-1]
		prevAfter := call.after
		call.after = func(err error) {
			if prevAfter != nil {
				prevAfter(err)
			}
			if err != nil {
				c.failTranscriptDetail(prev)
			}
		}
	} else if c.attachOperation == nil {
		c.failTranscriptDetail(prev)
	}
}

func (c *nativeClient) failTranscriptDetail(prev transcriptDetailNav) {
	if c.renderer.terminal == nil || c.renderer.terminal.Dialog == nil {
		c.restoreDetailNav()
		return
	}
	c.detailNav = prev
	c.detailNav.pendingDetail = false
	c.detailNav.pendingLast = false
	if prev.view != nil {
		if prev.active.Zero() {
			prev.view.ClearActiveDetail()
		} else {
			prev.view.SetActiveDetail(prev.active, false)
		}
	}
}

func (c *nativeClient) stepTranscriptDetail(step int) {
	nav := &c.detailNav
	if nav.view == nil || nav.active.Zero() || nav.pendingDetail {
		return
	}
	targets := nav.view.FeedTargets
	idx := slices.IndexFunc(targets, func(t activityui.DetailTarget) bool {
		return t.SameEvent(nav.active)
	})
	if idx < 0 {
		return
	}
	next := idx + step
	if next < 0 || next >= len(targets) {
		return
	}
	c.openTranscriptDetail(c.event.Frame.View.Version, nav.view, nav.thread, targets[next], step < 0)
}

func (c *nativeClient) clearDetailNavTarget() {
	if c.detailNav.view != nil {
		c.detailNav.view.ClearActiveDetail()
	}
	c.detailNav.active = activityui.DetailTarget{}
	c.detailNav.pendingLast = false
	c.detailNav.pendingDetail = false
}

func (c *nativeClient) restoreDetailNav() {
	nav := &c.detailNav
	if nav.view != nil {
		nav.view.ClearActiveDetail()
		if nav.hasSavedPos {
			if nav.savedFollow {
				nav.view.Follow()
			} else {
				nav.view.Following = false
				nav.view.Offset = nav.savedOffset
			}
		}
	}
	*nav = transcriptDetailNav{}
}

func (c *nativeClient) syncDetailNav(frame sessionapi.NativeFrame) {
	nav := &c.detailNav
	u := c.renderer.terminal
	if u == nil {
		return
	}
	if nav.view != nil && nav.view != u.Transcript && nav.view != u.Agents {
		*nav = transcriptDetailNav{}
		return
	}
	if nav.thread != "" && frame.View.Displayed.ID != "" && frame.View.Displayed.ID != nav.thread {
		if nav.view != nil {
			nav.view.ClearActiveDetail()
		}
		*nav = transcriptDetailNav{}
		return
	}
	if u.Dialog == nil || frame.Details == nil {
		if nav.hasSavedPos && !nav.pendingDetail {
			c.restoreDetailNav()
		}
		return
	}
	if nav.active.Zero() || nav.view == nil {
		return
	}
	if u.Dialog.Processes || u.Dialog.File != nil {
		c.clearDetailNavTarget()
		u.Dialog.EventNav = false
		return
	}
	u.Dialog.EventNav = true
	opened := nav.lastDetailID != u.Dialog.ID
	if opened {
		nav.lastDetailID = u.Dialog.ID
		nav.pendingDetail = false
		if nav.pendingLast && len(u.Dialog.Pages) > 0 {
			u.Dialog.ShowPage(len(u.Dialog.Pages) - 1)
		}
		nav.pendingLast = false
	}
	if len(u.Dialog.Pages) == 0 || u.Dialog.Page < 0 || u.Dialog.Page >= len(u.Dialog.Pages) {
		return
	}
	if nav.active.Snippet.Block == activityui.EditNavigationSnippet {
		page := u.Dialog.Pages[u.Dialog.Page]
		if page.Path != nav.active.Snippet.Path || page.DisplayPath != nav.active.DisplayPath {
			nav.active.Snippet.Path = page.Path
			nav.active.DisplayPath = page.DisplayPath
			nav.view.SetActiveDetail(nav.active, true)
		}
	}
}

func (c *nativeClient) openFeedbackDetails(text string) {
	pages := activity.DetailPages(activityui.Values([]activityui.Block{{Kind: "error", Body: text}}))
	c.localDetailID = c.localDetails.OpenDetails(c.event.Frame.View.Version, pages)
	// Local and server selections have distinct owners even if their counters
	// happen to match. Force the ordinary dialog's new-selection navigation path.
	c.renderer.terminal.Dialog = nil
}

func (c *nativeClient) closeLocalDetails() bool {
	if _, ok := c.localDetails.DetailView(c.localDetailID); !ok {
		return false
	}
	c.localDetails.CloseDetails(c.localDetailID)
	c.localDetailID = sessionpresentation.DetailID{}
	c.renderer.terminal.Dialog = nil
	return true
}
