package terminalview

import (
	"strings"

	"github.com/charmbracelet/x/ansi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffrender"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
)

// DetailTarget identifies one clickable transcript event in feed order, along
// with its row span in the laid-out feed.
type DetailTarget struct {
	Entry       uint64
	Source      uint64  // Resolved source entry sequence for single-source multi-segment events.
	Edit        bool    // True when this target opens an edit sheet rather than a command/entry sheet.
	Merged      bool    // True when this target is a merged multi-source block.
	Snippet     Snippet
	LastSnippet Snippet // Last distinct row snippet in this event (e.g. final file in a multi-file edit).
	DisplayPath string  // Optional alternate path for matching an active edit tab back to its transcript row.
	Start       int     // First feed row of this target.
	End         int     // Exclusive last feed row of this target.
}

type detailEventKey struct {
	entry       uint64
	source      uint64
	edit        bool
	mergedRun   uint64
	mergedBlock int
}

func (t DetailTarget) eventKey() detailEventKey {
	if t.Entry != 0 {
		return detailEventKey{entry: t.Entry}
	}
	if t.Edit || t.Snippet.Block == EditNavigationSnippet {
		source := t.Source
		if source == 0 {
			source = t.Snippet.Run
		}
		return detailEventKey{source: source, edit: true}
	}
	if !t.Merged && t.Source != 0 {
		return detailEventKey{source: t.Source}
	}
	return detailEventKey{mergedRun: t.Snippet.Run, mergedBlock: t.Snippet.Block}
}

// Zero reports whether the target names no transcript event.
func (t DetailTarget) Zero() bool {
	return t.Entry == 0 && t.Snippet == (Snippet{})
}

// SameEvent reports whether two targets open the same transcript event sheet.
func (t DetailTarget) SameEvent(other DetailTarget) bool {
	if t.Zero() || other.Zero() {
		return false
	}
	return t.eventKey() == other.eventKey()
}

// ResolveDetailTarget populates sheet-level identity fields from the current
// laid-out blocks so multi-segment commands share one event identity.
func (v *View) ResolveDetailTarget(target DetailTarget) DetailTarget {
	if target.Zero() || target.Entry != 0 {
		return target
	}
	if target.Snippet.Block == EditNavigationSnippet {
		target.Edit = true
		target.Source = target.Snippet.Run
		return target
	}
	if block, ok := v.SnippetBlock(target.Snippet); ok {
		if len(block.Members) > 0 {
			target.Merged = true
		} else if block.Source != 0 {
			target.Source = block.Source
		}
	}
	return target
}

// SetActiveDetail highlights the active transcript event (and optionally scrolls
// the transcript viewport to keep it visible while the detail sheet is open).
func (v *View) SetActiveDetail(target DetailTarget, reveal bool) {
	v.ActiveDetail = v.ResolveDetailTarget(target)
	if reveal {
		v.ActiveDetailReveal = true
	}
}

// ClearActiveDetail removes any active transcript event highlight.
func (v *View) ClearActiveDetail() {
	v.ActiveDetail = DetailTarget{}
	v.ActiveDetailReveal = false
}

func (v *View) collectDetailTargets(feed Feed) []DetailTarget {
	var targets []DetailTarget
	seen := make(map[detailEventKey]int)
	for row := range feed.Lines {
		var cand DetailTarget
		if row < len(feed.Questions) && feed.Questions[row] != 0 {
			cand = DetailTarget{Entry: feed.Questions[row], Start: row, End: row + 1}
		} else if row < len(feed.Snippets) && feed.Snippets[row] != (Snippet{}) {
			s := feed.Snippets[row]
			cand = v.ResolveDetailTarget(DetailTarget{Snippet: s, LastSnippet: s, Start: row, End: row + 1})
		} else {
			continue
		}
		key := cand.eventKey()
		if idx, exists := seen[key]; exists {
			if idx == len(targets)-1 {
				targets[idx].End = row + 1
			}
			if cand.Snippet != (Snippet{}) {
				targets[idx].LastSnippet = cand.Snippet
			}
			continue
		}
		seen[key] = len(targets)
		targets = append(targets, cand)
	}
	return targets
}

func (v *View) activeDetailRowMatch(feed Feed) (exactPath bool, start int, end int, found bool) {
	active := v.ActiveDetail
	if active.Zero() {
		return false, 0, 0, false
	}
	matchPath := func(s Snippet) bool {
		if s.Run != active.Snippet.Run || s.Block != EditNavigationSnippet {
			return false
		}
		if active.Snippet.Path != "" && s.Path == active.Snippet.Path {
			return true
		}
		return active.DisplayPath != "" && s.Path == active.DisplayPath
	}
	if active.Entry == 0 && active.Snippet.Block == EditNavigationSnippet && (active.Snippet.Path != "" || active.DisplayPath != "") {
		start, end = -1, -1
		for row := range feed.Lines {
			if row < len(feed.Snippets) && matchPath(feed.Snippets[row]) {
				if start < 0 {
					start = row
				}
				end = row + 1
			}
		}
		if start >= 0 {
			return true, start, end, true
		}
	}
	start, end = -1, -1
	for row := range feed.Lines {
		if v.rowMatchesDetailEvent(feed, row, active) {
			if start < 0 {
				start = row
			}
			end = row + 1
		}
	}
	return false, start, end, start >= 0
}

func (v *View) rowMatchesDetailEvent(feed Feed, row int, active DetailTarget) bool {
	if active.Entry != 0 {
		return row < len(feed.Questions) && feed.Questions[row] == active.Entry
	}
	if row < len(feed.Snippets) {
		s := feed.Snippets[row]
		if s == (Snippet{}) {
			return false
		}
		return v.ResolveDetailTarget(DetailTarget{Snippet: s}).SameEvent(active)
	}
	return false
}

func (v *View) rowMatchesActiveDetail(feed Feed, row int, active DetailTarget, exactPath bool) bool {
	if exactPath {
		if row >= len(feed.Snippets) {
			return false
		}
		s := feed.Snippets[row]
		if s.Run != active.Snippet.Run || s.Block != EditNavigationSnippet {
			return false
		}
		return active.Snippet.Path != "" && s.Path == active.Snippet.Path ||
			active.DisplayPath != "" && s.Path == active.DisplayPath
	}
	return v.rowMatchesDetailEvent(feed, row, active)
}

// highlightTranscriptGutter styles the two-cell left rail of an active
// transcript row using the same accent-on-selection palette as the file
// dialog's highlighted line-number gutter, preserving trailing SGR and OSC.
func highlightTranscriptGutter(line string, theme diffrender.Theme) string {
	style := theme.Accent() + theme.SelectionBackground()
	var out, sgr strings.Builder
	var state byte
	column := 0
	closed := false
	out.WriteString(style)
	for len(line) > 0 {
		seq, width, n, next := ansi.DecodeSequence(line, state, nil)
		line, state = line[n:], next
		if !closed && width == 0 && strings.HasPrefix(seq, "\x1b[") && strings.HasSuffix(seq, "m") {
			if seq == "\x1b[m" || seq == "\x1b[0m" {
				sgr.Reset()
			} else {
				sgr.WriteString(seq)
			}
			continue
		}
		if !closed && column+width > 2 {
			if column < 2 {
				out.WriteString(strings.Repeat(" ", 2-column))
			}
			out.WriteString(terminalui.Reset)
			out.WriteString(sgr.String())
			closed = true
		}
		out.WriteString(seq)
		if width > 0 {
			column += width
			if !closed && column == 2 {
				out.WriteString(terminalui.Reset)
				out.WriteString(sgr.String())
				closed = true
			}
		}
	}
	if !closed {
		if column < 2 {
			out.WriteString(strings.Repeat(" ", 2-column))
		}
		out.WriteString(terminalui.Reset)
	}
	return out.String()
}
