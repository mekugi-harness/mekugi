package presentation

import (
	"cmp"

	"fmt"
	"iter"
	"maps"
	"path/filepath"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/activitydetail"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/pathdisplay"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

type AttemptKey struct{ Workspace, Call string }

type AttemptIdentity struct {
	Key                 AttemptKey
	Change, Correlation string
	Stream              int
}

// Receipt is an immutable captured-effect projection. Calls retain their order:
// SourceAnchor selects the last witnessed writer; older receipts use the first
// host item. Siblings do not repeat its summary.
type Receipt struct {
	Thread string
	Calls  []string
	Text   string
	Pages  []activitydetail.Page

	SourceAnchor activitydetail.SourceAnchor
}

// AnchorCall preserves the historical default when no witnessed boundary joins
// this receipt's host membership.
func (r *Receipt) AnchorCall() string {
	if slices.Contains(r.Calls, r.SourceAnchor.Call) {
		return r.SourceAnchor.Call
	}
	if len(r.Calls) > 0 {
		return r.Calls[0]
	}
	return ""
}

type Attempt struct {
	AttemptIdentity
	Operation activitydetail.Operation
	Chunks    []Chunk
	Receipt   *Receipt
}

// HasAuthoredChanges excludes diagnostic detail chunks from edit workflows.
func (a Attempt) HasAuthoredChanges() bool {
	return slices.ContainsFunc(a.Chunks, func(chunk Chunk) bool { return changes.AuthoredReview(chunk.Review) })
}

type Counts struct {
	Callers map[string]diffmodel.Counts
	Net     *diffmodel.Counts
}

// Publication is detached, immutable saved-review content. Terminal selection
// and caller filtering cannot alter its unfiltered composition or totals.
type Publication struct {
	Files       []File
	Diagnostics []Chunk // Captured Git effects, excluded from authored Diff/counts.
	Unfiltered  map[string]File
	Counts      Counts
	initialized bool
}

// ReviewView joins admitted evidence with its attached workspace and coverage.
// Publication is immutable; revisions identify source replacement, not paint.
type ReviewView struct {
	Publication
	Workspace, Coverage, Failure string
	Revision                     uint64
}

// With derives one replacement from supplied captures, preserving stable order
// and recent-update marks. Consumers share the resulting unfiltered composition.
func (p Publication) With(files []File) Publication {
	v := View{Files: p.Files, Visible: p.Unfiltered, Initialized: p.initialized}
	v.Merge(files)
	v.RefreshVisible()
	return Publication{Files: v.Files, Diagnostics: p.Diagnostics, Unfiltered: v.Visible,
		Counts: Counts{Callers: CallerCounts(v.Files), Net: NetCounts(&v)}, initialized: true}
}

// ForDiff omits temporary-only captures and confirmed empty net files from the saved pane, without changing
// the retained publication used by details and edit totals. Boundary-crossing
// moves keep both endpoints so their non-temporary effects remain reviewable.
func (p Publication) ForDiff(workspace string) Publication {
	var captures []Chunk
	filtered := false
	for _, file := range p.Files {
		for _, chunk := range file.Chunks {
			onlyTemporary := chunk.Review.BeforePath != "" || chunk.Review.AfterPath != ""
			for _, path := range []string{chunk.Review.BeforePath, chunk.Review.AfterPath} {
				if path == "" {
					continue
				}
				if !filepath.IsAbs(path) {
					base := cmp.Or(chunk.Workspace, workspace)
					if !filepath.IsAbs(base) {
						onlyTemporary = false
						break
					}
					path = filepath.Join(base, path)
				}
				path = filepath.Clean(path)
				base := cmp.Or(chunk.Workspace, workspace)
				if filepath.IsAbs(base) {
					relative, err := filepath.Rel(base, path)
					if err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
						onlyTemporary = false
						break
					}
				}
				if path != "/tmp" && !strings.HasPrefix(path, "/tmp/") {
					onlyTemporary = false
					break
				}
			}
			if onlyTemporary {
				filtered = true
			} else {
				captures = append(captures, chunk)
			}
		}
	}
	if filtered {
		p = p.With(GroupCaptures(captures))
	}
	// Keep the complete publication for history, details and edit totals. A
	// zero line count alone cannot hide moves, empty creations or unknown bytes.
	// Link captures do not retain each endpoint's type, so identical rendered
	// text cannot prove that a regular-file/symlink replacement was reversed.
	var files []File
	var visible map[string]File
	for i, file := range p.Files {
		composed, known := p.Unfiltered[file.Key()]
		if known && !composed.incomplete && len(composed.Chunks) == 0 &&
			!slices.ContainsFunc(file.Chunks, func(chunk Chunk) bool { return chunk.Review.Link }) {
			if visible == nil {
				files = slices.Clone(p.Files[:i])
				visible = maps.Clone(p.Unfiltered)
			}
			delete(visible, file.Key())
		} else if visible != nil {
			files = append(files, file)
		}
	}
	if visible != nil {
		p.Files, p.Unfiltered = files, visible
	}
	return p
}

// Snapshot owns exact attempts and their one derived publication. Its contents
// remain immutable after Builder.Finish, including shared receipts and slices.
type Snapshot struct {
	Publication Publication
	attempts    map[AttemptKey]Attempt
	order       []AttemptKey
	receipts    []*Receipt // Immutable references, assembled only during hydration.

	// Negative positions block paths touched by structural or colliding groups.

}

func (s *Snapshot) Attempts() iter.Seq[Attempt] {
	return func(yield func(Attempt) bool) {
		if s != nil {
			for _, key := range s.order {
				if !yield(s.attempts[key]) {
					return
				}
			}
		}
	}
}

func (s *Snapshot) Len() int {
	if s == nil {
		return 0
	}
	return len(s.order)
}

func (s *Snapshot) Receipts() []*Receipt {
	if s == nil {
		return nil
	}
	return s.receipts
}

// Builder belongs to one unpublished hydration job. Previously accepted data
// is shared read-only; new attempts cannot escape a failed or stale job.
type Builder struct {
}

// ActivityDetail resolves the original captured invocation, never a composed
// successor or current workspace contents. Source: apps/mekugi/internal/router/native_edit_navigation.go:18:74@[5927a739].
func (s *Snapshot) ActivityDetail(request activitydetail.Request) activitydetail.Result {
	if request.Thread == "" || request.Item == "" {
		return activitydetail.Result{}
	}
	var result activitydetail.Result
	for attempt := range s.Attempts() {
		match := attempt.Operation == request.Operation
		if receipt := attempt.Receipt; receipt != nil {
			match = receipt.Thread == request.Thread && slices.Contains(receipt.Calls, request.Item)
		}
		if !match || len(attempt.Chunks) == 0 {
			continue
		}
		for i, chunk := range attempt.Chunks {
			workspace := cmp.Or(chunk.Workspace, attempt.Key.Workspace)
			target := pathdisplay.ForWorkspace(workspace, cmp.Or(chunk.Review.AfterPath, chunk.Review.BeforePath))
			page := activitydetail.Page{ID: "changehistory:" + cmp.Or(chunk.Key, fmt.Sprintf("%s\x00%s/%d", attempt.Key.Workspace, attempt.Key.Call, i)),
				Verb: chunk.Review.Action().Title(), Path: target, Language: "diff",
				Code: chunk.Review.UnifiedDiffWithPaths(pathdisplay.ForWorkspace(workspace, chunk.Review.BeforePath), pathdisplay.ForWorkspace(workspace, chunk.Review.AfterPath))}
			if page.Verb == "Move" {
				page.DisplayPath = pathdisplay.Move(workspace, chunk.Review.BeforePath, chunk.Review.AfterPath)
			}
			result.Pages = append(result.Pages, page)
			if result.Selected == "" && (request.Path == "" || request.Path == target ||
				chunk.Review.Action().Title() == "Move" && request.Path == pathdisplay.Move(workspace, chunk.Review.BeforePath, chunk.Review.AfterPath) ||
				request.Directory && strings.HasPrefix(target, strings.TrimSuffix(request.Path, "/")+"/")) {
				result.Selected = page.ID
			}
		}
	}
	if result.Selected == "" {
		return activitydetail.Result{}
	}
	return result
}
