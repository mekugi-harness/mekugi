package presentation

import (
	"cmp"
	"maps"
	"path/filepath"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
)

func authoredReview(file diffmodel.ReviewFile) bool {
	return file.Origin == "" && file.OriginNote != "observed during command window"
}

func gitDiagnosticReview(file diffmodel.ReviewFile) bool {
	return file.Origin == "git" || strings.HasPrefix(file.Origin, "git ")
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
