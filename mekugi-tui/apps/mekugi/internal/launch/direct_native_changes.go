package launch

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"maps"
	"path/filepath"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/pathdisplay"
	activityfeed "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

type nativeEditItem struct{ thread, turn, item string }

func isWorkspaceScope(scope string) bool {
	return scope == "" || scope == "workspace"
}

type nativeCapturedFile struct {
	chunk          changes.Chunk
	path           string
	scope          string
	added, removed int
}

type directChangeOrigin struct {
	ChangeID       string  `json:"changeId"`
	Sequence       uint64  `json:"sequence"`
	ThreadID       string  `json:"threadId"`
	AuthorThreadID *string `json:"authorThreadId"`
	SourceLabel    string  `json:"sourceLabel"`
	TurnID         string  `json:"turnId"`
	ItemID         string  `json:"itemId"`
	Workspace      string  `json:"workspace"`
}

type directThreadChangesResponse struct {
	Workspace  string               `json:"workspace"`
	Incomplete bool                 `json:"incomplete"`
	Changes    []directChangeOrigin `json:"changes"`
	Files      []directChangeFile   `json:"files"`
	NextCursor string               `json:"nextCursor"`
}

type directChangeFile struct {
	Path          string   `json:"path"`
	AbsolutePath  string   `json:"absolutePath"`
	ChangeIDs     []string `json:"changeIds"`
	MovedFrom     string   `json:"movedFrom"`
	Status        string   `json:"status"`
	UnifiedDiff   string   `json:"unifiedDiff"`
	Additions     int      `json:"additions"`
	Deletions     int      `json:"deletions"`
	IsBinary      bool     `json:"isBinary"`
	CoverageNotes []string `json:"coverageNotes"`
	Scope         string   `json:"scope"`
}

func (c *directNativeConnection) requestChanges(threadID string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if displayed := cmp.Or(c.displayedThread, c.mainThread); displayed == "" ||
		(threadID != "" && threadID != displayed) {
		return
	}
	select {
	case c.reviewPending <- struct{}{}:
	default:
	}
}

func (c *directNativeConnection) reviewLoop() {
	for {
		select {
		case <-c.ctx.Done():
			return
		case <-c.reviewPending:
			c.mu.Lock()
			thread := cmp.Or(c.displayedThread, c.mainThread)
			c.reviewRev++
			rev := c.reviewRev
			c.mu.Unlock()
			c.refreshChanges(thread, rev)
		}
	}
}

func (c *directNativeConnection) readChanges(ctx context.Context, threadID string) (directThreadChangesResponse, error) {
	var response directThreadChangesResponse
	var cursor string
	var patchBytes int
	seen := make(map[string]bool)
	for {
		params := map[string]any{
			"threadId":         threadID,
			"perCapture":       true,
			"includeExternal":  true,
			"includeEphemeral": true,
			"limit":            100,
		}
		if cursor != "" {
			params["cursor"] = cursor
		}
		var page directThreadChangesResponse
		if err := c.wire.Call(ctx, "thread/changes/read", params, &page); err != nil {
			return directThreadChangesResponse{}, err
		}
		response.Workspace = cmp.Or(response.Workspace, page.Workspace)
		response.Incomplete = response.Incomplete || page.Incomplete
		response.Changes = append(response.Changes, page.Changes...)
		for _, file := range page.Files {
			patchBytes += len(file.UnifiedDiff)
			if patchBytes > diffrender.MaxSourceBytes {
				return directThreadChangesResponse{}, errors.New("saved diff exceeds the terminal renderer's source limit")
			}
			if last := len(response.Files) - 1; last >= 0 &&
				response.Files[last].AbsolutePath == file.AbsolutePath &&
				slices.Equal(response.Files[last].ChangeIDs, file.ChangeIDs) {
				response.Files[last].UnifiedDiff += file.UnifiedDiff
			} else {
				response.Files = append(response.Files, file)
			}
		}
		if page.NextCursor == "" {
			return response, nil
		}
		if seen[page.NextCursor] {
			return directThreadChangesResponse{}, errors.New("saved diff cursor did not advance")
		}
		seen[page.NextCursor] = true
		cursor = page.NextCursor
	}
}

func (c *directNativeConnection) refreshChanges(threadID string, rev uint64) {
	resp, err := c.readChanges(c.ctx, threadID)
	c.mu.Lock()
	if c.closed || c.ctx.Err() != nil || threadID != cmp.Or(c.displayedThread, c.mainThread) {
		c.mu.Unlock()
		return
	}
	if err != nil {
		if c.review == nil || c.reviewThread != threadID {
			c.review = &changes.ReviewView{Workspace: c.workspace}
		}
		failed := *c.review
		failed.Failure = err.Error()
		failed.Revision = rev
		c.review, c.reviewThread = &failed, threadID
		c.publishLocked()
		c.mu.Unlock()
		return
	}

	var pub changes.Publication
	if c.review != nil && c.reviewThread == threadID {
		pub = c.review.Publication
	}
	ws := cmp.Or(resp.Workspace, c.workspace)
	callers := maps.Clone(c.threadCallers)
	c.mu.Unlock()

	origins := make(map[string]directChangeOrigin, len(resp.Changes))
	for _, origin := range resp.Changes {
		origins[origin.ChangeID] = origin
	}
	captures := make([]changes.Chunk, 0, len(resp.Files))
	receipts := make(map[nativeEditItem][]nativeCapturedFile)
	for _, f := range resp.Files {
		// Each file here is one capture. Composed results are not operation receipts.
		if len(f.ChangeIDs) != 1 {
			continue
		}
		origin, ok := origins[f.ChangeIDs[0]]
		if !ok {
			continue
		}
		workspace := cmp.Or(origin.Workspace, ws)
		path := cmp.Or(f.AbsolutePath, f.Path)
		beforePath, afterPath := path, path
		if f.MovedFrom != "" {
			beforePath = f.MovedFrom
			if !filepath.IsAbs(beforePath) && filepath.IsAbs(workspace) {
				beforePath = filepath.Join(workspace, beforePath)
			}
		}
		switch f.Status {
		case "A":
			beforePath = ""
		case "D":
			afterPath = ""
		}
		caller := ""
		if origin.AuthorThreadID != nil {
			id := *origin.AuthorThreadID
			caller = cmp.Or(callers[id], id)
		}
		chunk := changes.Chunk{
			Key:          origin.ChangeID + ":" + path,
			Workspace:    workspace,
			CaptureOrder: origin.Sequence,
			Review: diffmodel.ReviewFile{
				BeforePath: beforePath, AfterPath: afterPath, Diff: f.UnifiedDiff,
				Binary: f.IsBinary, Incomplete: strings.Join(f.CoverageNotes, "; "),
			},
			Origin: changes.Origin{
				Change: origin.ChangeID, Caller: caller, Source: origin.SourceLabel,
			},
		}
		// Saved Diff defaults to Workspace edits; External/Ephemeral remain in
		// ChangeHistoryStore for mchanges and compact transcript summaries.
		if isWorkspaceScope(f.Scope) {
			captures = append(captures, chunk)
		}
		if origin.ItemID != "" {
			key := nativeEditItem{origin.ThreadID, origin.TurnID, origin.ItemID}
			receipts[key] = append(receipts[key], nativeCapturedFile{
				chunk: chunk, path: path, scope: f.Scope, added: f.Additions, removed: f.Deletions,
			})
		}
	}

	review := &changes.ReviewView{
		Publication: pub.With(changes.GroupCaptures(captures)),
		Workspace:   ws,
		Revision:    rev,
	}
	if resp.Incomplete {
		review.Coverage = "Partial capture"
	}
	for _, captures := range receipts {
		slices.SortStableFunc(captures, func(a, b nativeCapturedFile) int {
			return cmp.Compare(a.chunk.CaptureOrder, b.chunk.CaptureOrder)
		})
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed || c.ctx.Err() != nil || threadID != cmp.Or(c.displayedThread, c.mainThread) {
		return
	}
	c.reviewThread, c.review = threadID, review

	for key := range c.editReceipts {
		if _, exists := receipts[key]; !exists {
			if rec := c.itemsByID[directItemKey{key.thread, key.item}]; rec != nil && rec.turnID == key.turn {
				c.setEditReceiptLocked(rec, nil)
			}
		}
	}
	c.editReceipts = receipts
	for key, captures := range receipts {
		if rec := c.itemsByID[directItemKey{key.thread, key.item}]; rec != nil && rec.turnID == key.turn {
			c.setEditReceiptLocked(rec, captures)
		}
	}
	c.publishLocked()
}

func (c *directNativeConnection) setEditReceiptLocked(rec *directItemRecord, files []nativeCapturedFile) {
	if rec.itemType != "commandExecution" || len(rec.entry.Values) == 0 {
		return
	}
	var rows []activity.ChangeRow
	var externalCount int
	var externalIDs []string
	for _, file := range files {
		chunk := file.chunk
		if !isWorkspaceScope(file.scope) {
			externalCount++
			if chunk.Change != "" && !slices.Contains(externalIDs, chunk.Change) {
				externalIDs = append(externalIDs, chunk.Change)
			}
			continue
		}
		rows = append(rows, activity.ChangeRow{
			Verb:  chunk.Review.Action().Title(),
			Label: pathdisplay.ForWorkspace(chunk.Workspace, file.path),
			Added: file.added, Removed: file.removed,
			Note: capturedFileNote(chunk),
		})
	}
	if externalCount > 0 {
		noun := "external files"
		if externalCount == 1 {
			noun = "external file"
		}
		note := ""
		if len(externalIDs) > 0 {
			note = "mchanges " + strings.Join(externalIDs, " ") + " --summary"
		}
		rows = append(rows, activity.ChangeRow{
			Verb:  "Captured",
			Label: fmt.Sprintf("%d %s", externalCount, noun),
			Note:  note,
		})
	}
	index := len(rec.entry.Values) - 1
	value := rec.entry.Values[index]
	if slices.Equal(value.Changes, rows) {
		return
	}
	value.Changes = rows
	rec.entry.Values = slices.Clone(rec.entry.Values)
	rec.entry.Values[index] = value
	rec.entry.Revision++
	c.refreshCommandDetailsLocked(rec)
}

func capturedFileNote(chunk changes.Chunk) string {
	note := chunk.Source + " · " + chunk.Change
	if chunk.Review.Incomplete != "" {
		note += " · " + chunk.Review.Incomplete
	}
	if chunk.Review.Binary {
		note += " · Binary"
	}
	return note
}

func (c *directNativeConnection) capturedDetailValuesLocked(rec *directItemRecord) []activity.Value {
	key := nativeEditItem{rec.threadID, rec.turnID, rec.id}
	captures := c.editReceipts[key]
	values := make([]activity.Value, 0, len(captures))
	for _, file := range captures {
		chunk := file.chunk
		values = append(values, activity.Value{
			Path: file.path, Workdir: chunk.Workspace, Effect: true, Exited: true,
			Content: activityfeed.Content{
				Kind: "op", Verb: chunk.Review.Action().Title(),
				Label: pathdisplay.ForWorkspace(chunk.Workspace, file.path),
				Body:  capturedFileNote(chunk),
				Code:  chunk.Review.Diff, Lang: "diff",
			},
		})
	}
	return values
}
