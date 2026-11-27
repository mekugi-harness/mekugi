package launch

import (
	"context"
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activityfeed "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

func processPageID(p sessionpresentation.Process) string {
	return "process:" + p.Thread + ":" + p.ID + ":" + p.Turn + ":" + p.Item
}

func (c *directNativeConnection) listThreadProcesses(ctx context.Context, thread, caller string) ([]sessionpresentation.Process, error) {
	var response struct {
		Data []struct {
			Turn    string `json:"turnId"`
			Item    string `json:"itemId"`
			Process string `json:"processId"`
			Command string `json:"command"`
		} `json:"data"`
	}
	if err := c.wire.Call(ctx, "thread/backgroundTerminals/list", map[string]string{"threadId": thread}, &response); err != nil {
		return nil, err
	}
	processes := make([]sessionpresentation.Process, 0, len(response.Data))
	for _, item := range response.Data {
		processes = append(processes, sessionpresentation.Process{
			Thread: thread, Turn: item.Turn, Item: item.Item, ID: item.Process,
			Agent: caller, Command: unwrapDirectShellCommand(item.Command),
		})
	}
	return processes, nil
}

func (c *directNativeConnection) refreshProcesses(ctx context.Context, root string) error {
	c.mu.Lock()
	if root == "" || root != c.mainThread {
		c.mu.Unlock()
		return nil
	}
	c.processRefresh++
	refresh := c.processRefresh
	threads := []string{root}
	if c.family != nil && c.family.Root == root {
		for _, thread := range c.family.Threads {
			if thread != root {
				threads = append(threads, thread)
			}
		}
	}
	callers := make(map[string]string, len(threads))
	for _, thread := range threads {
		callers[thread] = c.threadCallers[thread]
	}
	c.mu.Unlock()

	byThread := make(map[string][]sessionpresentation.Process, len(threads))
	for _, thread := range threads {
		processes, err := c.listThreadProcesses(ctx, thread, callers[thread])
		if err != nil {
			if thread == root || ctx.Err() != nil {
				return err
			}
			// An unreadable or unloaded descendant does not hide the rest.
			continue
		}
		byThread[thread] = processes
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	if c.mainThread != root || c.processRefresh != refresh || ctx.Err() != nil {
		return nil
	}
	var processes []sessionpresentation.Process
	c.processLists = make(map[string]bool, len(byThread))
	for _, thread := range threads {
		listed, ok := byThread[thread]
		c.processLists[thread] = ok
		if !ok {
			for _, previous := range c.processes {
				if previous.Thread == thread {
					listed = append(listed, previous)
				}
			}
		}
		for _, process := range listed {
			rec := c.processRecordLocked(process)
			if rec == nil || !rec.entry.OutputComplete {
				processes = append(processes, process)
			}
		}
	}
	if !slices.Equal(c.processes, processes) {
		c.processes = processes
		c.updateProcessDetailsLocked()
		c.publishLocked()
	}
	return nil
}

func (c *directNativeConnection) invalidateProcessListLocked(thread string) {
	c.processRefresh++
	delete(c.processLists, thread)
}

func (c *directNativeConnection) openProcessDetails(ctx context.Context, req sessionapi.NativeDetailRequest) error {
	c.mu.Lock()
	root := c.mainThread
	c.mu.Unlock()
	if err := c.refreshProcesses(ctx, root); err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if root == "" || root != c.mainThread {
		return sessionpresentation.ErrStaleIntent
	}
	c.detailSeq++
	id := sessionpresentation.DetailID{Attachment: c.attachID, Selection: c.detailSeq}
	c.details = &sessionapi.NativeDetails{
		Source: sessionpresentation.Thread{ID: root, Caller: c.threadCallers[root]},
		Agent:  req.Agent,
		View:   sessionpresentation.DetailView{ID: id, Processes: true},
	}
	c.updateProcessDetailsLocked()
	c.publishLocked()
	return nil
}

func (c *directNativeConnection) processRecordLocked(process sessionpresentation.Process) *directItemRecord {
	for _, rec := range slices.Backward(c.items[process.Thread]) {
		if rec.id == process.Item && rec.turnID == process.Turn && rec.itemType == "commandExecution" {
			return rec
		}
	}
	return nil
}

func (c *directNativeConnection) updateProcessDetailsLocked() {
	if c.details == nil || !c.details.View.Processes {
		return
	}
	details := *c.details
	running := make(map[string]sessionpresentation.Process, len(c.processes))
	for _, process := range c.processes {
		running[processPageID(process)] = process
	}
	pages := make([]sessionpresentation.DetailItem, 0, len(details.View.Pages)+len(running))
	appendProcess := func(page sessionpresentation.DetailItem, active bool) {
		process := *page.Process
		rec := c.processRecordLocked(process)
		var sourceVer uint64
		if rec != nil {
			sourceVer = rec.entry.Revision
		}
		if page.OutputVersion == 0 || page.Revision != sourceVer || page.Running != active {
			if rec != nil && len(rec.entry.Values) != 0 {
				page.Value = rec.entry.Values[len(rec.entry.Values)-1]
			}
			if rec != nil {
				page.Source = rec.seq
				page.Tail, page.TailOmitted = directCommandTail(rec.output)
				page.Retained = rec.output != ""
			}
			page.Running, page.OutputLive = active, active
			page.Owner = process.Agent
			page.Revision = sourceVer
			page.OutputVersion++
		}
		if details.Output != nil && details.Output.Page == page.ID && details.Output.Version != page.OutputVersion {
			details.Output = c.buildDetailOutputLocked(details.View.ID, page.ID, page.OutputVersion, rec, page.DetailPage)
		}
		pages = append(pages, page)
	}
	for _, page := range details.View.Pages {
		if page.Process != nil {
			_, active := running[page.ID]
			appendProcess(page, active)
			delete(running, page.ID)
		}
	}
	for _, process := range c.processes {
		id := processPageID(process)
		if _, ok := running[id]; ok {
			appendProcess(sessionpresentation.DetailItem{
				Process: &process,
				DetailPage: activity.DetailPage{
					ID: id, Value: activity.Value{
						Content: activityfeed.Content{Kind: "op", Verb: "Run", Code: process.Command},
					},
				},
			}, true)
		}
	}
	if len(pages) == 0 {
		pages = append(pages, sessionpresentation.DetailItem{
			DetailPage: activity.DetailPage{ID: "process:empty"},
		})
	}
	if details.Selected == "" || details.Selected == "process:empty" {
		details.Selected = pages[0].ID
	}
	details.View.Pages = pages
	details.View.Revision++
	c.details = &details
}

func (c *directNativeConnection) handleStopProcess(ctx context.Context, proc sessionpresentation.Process) error {
	c.mu.Lock()
	root := c.mainThread
	known := proc.Turn != "" && slices.ContainsFunc(c.processes, func(active sessionpresentation.Process) bool {
		return processPageID(active) == processPageID(proc)
	})
	c.mu.Unlock()
	if !known {
		return sessionpresentation.ErrStaleIntent
	}
	terminated, err := c.terminateThreadProcess(ctx, proc)
	if err != nil {
		return err
	}
	if !terminated {
		_ = c.refreshProcesses(ctx, root)
		return sessionpresentation.ErrStaleIntent
	}
	c.mu.Lock()
	c.removeLiveProcessLocked(proc)
	c.publishLocked()
	c.mu.Unlock()
	return nil
}

func (c *directNativeConnection) terminateThreadProcess(ctx context.Context, proc sessionpresentation.Process) (bool, error) {
	var response struct {
		Terminated bool `json:"terminated"`
	}
	err := c.wire.Call(ctx, "thread/backgroundTerminals/terminate", map[string]any{
		"threadId": proc.Thread, "processId": proc.ID,
		"invocation": map[string]string{"turnId": proc.Turn, "itemId": proc.Item},
	}, &response)
	return response.Terminated, err
}

func (c *directNativeConnection) removeLiveProcessLocked(proc sessionpresentation.Process) {
	// Invalidate an in-flight listing taken before the acknowledged stop.
	c.processRefresh++
	c.processes = slices.DeleteFunc(c.processes, func(active sessionpresentation.Process) bool {
		return processPageID(active) == processPageID(proc)
	})
	c.updateProcessDetailsLocked()
}
