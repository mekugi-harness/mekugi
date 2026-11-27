package launch

import (
	"cmp"
	"context"
	"path/filepath"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/pathdisplay"
	activityfeed "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

type nativeEditItem struct{ thread, turn, item string }

type nativeCapturedFile struct {
	chunk          changes.Chunk
	path           string
	added, removed int
}

type directChangeOrigin struct {
	ChangeID       string  `json:"changeId"`
	Sequence       uint64  `json:"sequence"`
	ThreadID       string  `json:"threadId"`
	AuthorThreadID *string `json:"authorThreadId"`
	SourceKind     string  `json:"sourceKind"`
	TurnID         string  `json:"turnId"`
	ItemID         string  `json:"itemId"`
	Workspace      string  `json:"workspace"`
}

type directThreadChangesResponse struct {
	Workspace  string               `json:"workspace"`
	Incomplete bool                 `json:"incomplete"`
	Changes    []directChangeOrigin `json:"changes"`
	Files      []struct {
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
	} `json:"files"`
}

func capturedSource(kind string) string {
	switch kind {
	case "ApplyPatch":
		return "apply_patch"
	case "ShellWrite":
		return "Shell"
	case "UserShell":
		return "User shell"
	case "MchangesApply":
		return "mchanges apply"
	case "MchangesRevert":
		return "mchanges revert"
	default:
		return kind
	}
}

func (c *directNativeConnection) refreshChanges(ctx context.Context, threadID string) {
	c.mu.Lock()
	if threadID != cmp.Or(c.displayedThread, c.mainThread) {
		c.mu.Unlock()
		return
	}
	c.reviewRev++
	rev := c.reviewRev
	c.mu.Unlock()

	var resp directThreadChangesResponse
	if err := c.wire.Call(ctx, "thread/changes/read", map[string]any{
		"threadId": threadID, "perCapture": true,
	}, &resp); err != nil {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if rev != c.reviewRev || threadID != cmp.Or(c.displayedThread, c.mainThread) {
		return
	}

	origins := make(map[string]directChangeOrigin, len(resp.Changes))
	for _, origin := range resp.Changes {
		origins[origin.ChangeID] = origin
	}
	ws := cmp.Or(resp.Workspace, c.workspace)
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
			caller = cmp.Or(c.threadCallers[id], id)
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
				Change: origin.ChangeID, Caller: caller, Source: capturedSource(origin.SourceKind),
			},
		}
		captures = append(captures, chunk)
		if origin.ItemID != "" {
			key := nativeEditItem{origin.ThreadID, origin.TurnID, origin.ItemID}
			receipts[key] = append(receipts[key], nativeCapturedFile{
				chunk: chunk, path: path, added: f.Additions, removed: f.Deletions,
			})
		}
	}

	var pub changes.Publication
	if c.review != nil && c.reviewThread == threadID {
		pub = c.review.Publication
	}
	c.reviewThread = threadID
	c.review = &changes.ReviewView{
		Publication: pub.With(changes.GroupCaptures(captures)),
		Workspace:   ws,
		Revision:    rev,
	}
	if resp.Incomplete {
		c.review.Coverage = "Partial capture"
	}

	for key := range c.editReceipts {
		if _, exists := receipts[key]; !exists {
			if rec := c.itemsByID[key.item]; rec != nil && rec.threadID == key.thread && rec.turnID == key.turn {
				c.setEditReceiptLocked(rec, nil)
			}
		}
	}
	c.editReceipts = receipts
	for key, captures := range receipts {
		slices.SortStableFunc(captures, func(a, b nativeCapturedFile) int {
			return cmp.Compare(a.chunk.CaptureOrder, b.chunk.CaptureOrder)
		})
		if rec := c.itemsByID[key.item]; rec != nil && rec.threadID == key.thread && rec.turnID == key.turn {
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
	for _, file := range files {
		chunk := file.chunk
		rows = append(rows, activity.ChangeRow{
			Verb:  chunk.Review.Action().Title(),
			Label: pathdisplay.ForWorkspace(chunk.Workspace, file.path),
			Added: file.added, Removed: file.removed,
			Note: capturedFileNote(chunk),
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
