package uisnapshot

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/pmezard/go-difflib/difflib"
)

// RenderReviewFile builds a diffmodel.ReviewFile fixture from before/after strings for tests.
func RenderReviewFile(beforePath, afterPath, before, after string) diffmodel.ReviewFile {
	file := diffmodel.ReviewFile{BeforePath: beforePath, AfterPath: afterPath}
	splitLines := func(content string) []string {
		if content == "" {
			return nil
		}
		lines := strings.SplitAfter(content, "\n")
		if lines[len(lines)-1] == "" {
			lines = lines[:len(lines)-1]
		}
		return lines
	}
	a, b := splitLines(before), splitLines(after)
	groups := difflib.NewMatcher(a, b).GetGroupedOpCodes(10)
	var diff strings.Builder
	fmt.Fprintf(&diff, "%s %q -> %q\n", file.Action(), file.BeforePath, file.AfterPath)
	if len(groups) != 0 {
		pathLabel := func(p string) string {
			if p == "" {
				return "/dev/null"
			}
			return strconv.Quote(p)
		}
		fmt.Fprintf(&diff, "--- %s\n+++ %s\n", pathLabel(file.BeforePath), pathLabel(file.AfterPath))
	}
	rangeLabel := func(start, end int) string {
		count := end - start
		if count == 0 {
			return fmt.Sprintf("%d,0", start)
		}
		if count == 1 {
			return fmt.Sprintf("%d", start+1)
		}
		return fmt.Sprintf("%d,%d", start+1, count)
	}
	writeLines := func(prefix byte, lines []string) {
		for _, line := range lines {
			diff.WriteByte(prefix)
			diff.WriteString(line)
			if !strings.HasSuffix(line, "\n") {
				diff.WriteString("\n\\ No newline at end of file\n")
			}
		}
	}
	for _, group := range groups {
		first, last := group[0], group[len(group)-1]
		fmt.Fprintf(&diff, "@@ -%s +%s @@\n", rangeLabel(first.I1, last.I2), rangeLabel(first.J1, last.J2))
		for _, op := range group {
			if op.Tag == 'e' {
				writeLines(' ', a[op.I1:op.I2])
			}
			if op.Tag == 'r' || op.Tag == 'd' {
				writeLines('-', a[op.I1:op.I2])
			}
			if op.Tag == 'r' || op.Tag == 'i' {
				writeLines('+', b[op.J1:op.J2])
			}
		}
	}
	file.Diff = diff.String()
	return file
}
