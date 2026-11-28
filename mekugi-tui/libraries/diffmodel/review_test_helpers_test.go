package diffmodel

import (
	"slices"
	"strings"

	"github.com/pmezard/go-difflib/difflib"
)

func reviewLines(content string) []string {
	if content == "" {
		return nil
	}
	lines := strings.SplitAfter(content, "\n")
	if lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	return lines
}

func RenderReviewFile(beforePath, afterPath, before, after string) ReviewFile {
	a, b := reviewLines(before), reviewLines(after)
	const contextLines = 10
	return renderReviewGroups(ReviewFile{BeforePath: beforePath, AfterPath: afterPath}, a, b, 0, 0, difflib.NewMatcher(a, b).GetGroupedOpCodes(contextLines))
}

func (c *ReviewComposition) Files() []ReviewFile {
	projection := *c
	projection.regions = nil
	for _, region := range c.regions {
		if n := len(projection.regions); n > 0 {
			last := &projection.regions[n-1]
			if last.hidden == region.hidden &&
				last.beforeStart+len(last.before) == region.beforeStart &&
				last.afterStart+len(last.after) == region.afterStart {
				last.before = append(last.before, region.before...)
				last.after = append(last.after, region.after...)
				continue
			}
		}
		region.before, region.after = slices.Clone(region.before), slices.Clone(region.after)
		projection.regions = append(projection.regions, region)
	}
	highlighted := projection.FilesWithHighlights()
	files := make([]ReviewFile, len(highlighted))
	for i, file := range highlighted {
		files[i] = file.ReviewFile
	}
	return files
}

func (c *ReviewComposition) Consistent(content string) bool {
	lines := reviewLines(content)
	for position, line := range c.current {
		if position >= len(lines) || lines[position] != line {
			return false
		}
	}
	return true
}
