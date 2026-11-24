package terminalview

import (
	"path/filepath"
	"slices"
	"strings"

	pathterminal "github.com/mekugi-harness/mekugi/mekugi-tui/libraries/pathdisplay/terminal"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/sourceanalysis/markdownsyntax"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/terminalui"
)

// workdirGroups labels runs of at least two visible events with the same
// non-default directory. A neighboring edit can join when its target is
// within that directory, even if the host patch did not report a workdir.
func workdirGroups(blocks []Block) []string {
	first := slices.IndexFunc(blocks, func(b Block) bool { return b.Workdir != "" })
	if first < 0 {
		return nil
	}
	dirs := make([]string, len(blocks))
	for i := first; i < len(blocks); i++ {
		b := blocks[i]
		dirs[i] = b.Workdir
		if dirs[i] == "" && b.Kind == "filter" && i > 0 && b.Source == blocks[i-1].Source {
			dirs[i] = dirs[i-1]
		}
	}
	for i := first; i < len(blocks); {
		dir := dirs[i]
		if dir == "" {
			i++
			continue
		}
		adoptWorkdirEdits(blocks, dirs, i-1, -1, dir)
		for i < len(blocks) && dirs[i] == dir {
			i = adoptWorkdirEdits(blocks, dirs, i+1, 1, dir)
		}
	}
	for start := 0; start < len(dirs); {
		end := start + 1
		multiple := mergedWorkdirEvents(blocks[start])
		for end < len(dirs) && dirs[end] == dirs[start] {
			multiple = multiple || blocks[end].Source != blocks[start].Source || mergedWorkdirEvents(blocks[end])
			end++
		}
		if !multiple {
			clear(dirs[start:end])
		}
		start = end
	}
	return dirs
}

// An edit event belongs under the heading only if all its visible paths do.
func adoptWorkdirEdits(blocks []Block, dirs []string, at, step int, dir string) int {
	unassigned := func(i int) bool { return i >= 0 && i < len(dirs) && dirs[i] == "" }
	for unassigned(at) {
		end := at
		for unassigned(end) && blocks[end].Source == blocks[at].Source {
			if editWithin(blocks[end], dir) == "" {
				return at
			}
			end += step
		}
		for at != end {
			dirs[at] = dir
			at += step
		}
	}
	return at
}

func mergedWorkdirEvents(block Block) bool {
	for _, member := range block.Members {
		if member.Source != block.Source {
			return true
		}
	}
	return false
}

func groupedWorkdir(groups []string, i int) string {
	if groups == nil {
		return ""
	}
	return groups[i]
}

func workdirHeading(dir string, width int) string {
	const label = "In "
	return terminalui.Dim + label + terminalui.Undim +
		terminalui.StyledPath(pathterminal.Truncate(dir, width-len(label)))
}

// paintInWorkdir changes the painted label only. The block in Run.Blocks and
// the edit snippet still address the original source and workspace path.
func paintInWorkdir(block Block, dir string) Block {
	if dir != "" {
		block.ShowWorkdir = false
		if path := editWithin(block, dir); path != "" {
			_, end, _ := markdownsyntax.CodeSpan(block.Label, 0)
			block.Label = markdownsyntax.QuoteCode(path) + block.Label[end:]
		}
	}
	return block
}

func editWithin(block Block, dir string) string {
	if !block.EditOperation() {
		return ""
	}
	trim := func(path string) string {
		prefix := strings.TrimRight(dir, string(filepath.Separator)) + string(filepath.Separator)
		if rel, ok := strings.CutPrefix(path, prefix); ok && filepath.IsLocal(rel) && filepath.Clean(rel) != "." {
			return rel
		}
		return ""
	}
	path := block.EditPath()
	if before, after, ok := strings.Cut(path, " => "); ok && block.Verb == "Move" {
		before, after = trim(before), trim(after)
		if before != "" && after != "" {
			return before + " => " + after
		}
		return ""
	}
	return trim(path)
}
