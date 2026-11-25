package launch

import (
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/attachments"
)

// markdownFileTarget resolves local destinations lexically. Relative paths
// require the selected workspace metadata.
func markdownFileTarget(workspace, target string) (selection markdownFileSelection, ok bool) {
	target = strings.TrimPrefix(target, "@")
	path := target
	if strings.HasPrefix(target, "file:") {
		parsed, err := url.Parse(target)
		if err != nil || parsed.Host != "" && parsed.Host != "localhost" || parsed.RawQuery != "" || parsed.Opaque != "" {
			return
		}
		path = parsed.Path
		if parsed.Fragment != "" {
			first, last, valid := attachments.ParseLineSpan(parsed.Fragment)
			if !valid {
				return
			}
			selection.location, selection.first, selection.last = "#"+parsed.Fragment, first, last
		}
	} else {
		candidate, _, _, _ := markdownLinkLocation(target)
		if !filepath.IsAbs(candidate) && (candidate == "" || strings.HasPrefix(candidate, "#") || strings.HasPrefix(candidate, "?") || strings.Contains(strings.SplitN(candidate, "/", 2)[0], ":")) {
			return
		}
	}
	if strings.Contains(path, "://") || path == "" || strings.ContainsAny(path, "\x00\r\n\x1b") {
		return
	}
	if !filepath.IsAbs(path) {
		if !filepath.IsAbs(workspace) {
			return
		}
		path = filepath.Join(workspace, path)
	}
	selection.path = path
	return selection, true
}

type markdownFileSelection struct {
	path, location string
	first, last    int
}

// Raw destinations can be literal filenames. Strip a location only if the
// literal name is absent. An explicit file URL fragment is already separate.
func (s markdownFileSelection) stat() (markdownFileSelection, os.FileInfo, error) {
	info, err := os.Stat(s.path)
	if s.first == 0 && os.IsNotExist(err) {
		base, location, first, last := markdownLinkLocation(s.path)
		if first > 0 {
			s = markdownFileSelection{path: base, location: location, first: first, last: last}
			info, err = os.Stat(base)
		}
	}
	return s, info, err
}

// Capture only detached workspace metadata for lexical rendering admission.
func markdownFileLinks(workspace string) func(string) bool {
	return func(target string) bool {
		_, ok := markdownFileTarget(workspace, target)
		return ok
	}
}

// detailFiles bounds asynchronous acquisition independently of live session control.
// The physical slot survives attachment replacement until its read completes.
func markdownLinkLocation(path string) (base, location string, first, last int) {
	if hash := strings.LastIndexByte(path, '#'); hash >= 0 {
		if first, last, ok := attachments.ParseLineSpan(path[hash+1:]); ok {
			return path[:hash], path[hash:], first, last
		}
	}
	colon := strings.LastIndexByte(path, ':')
	if colon < 0 {
		return path, "", 0, 0
	}
	from, to, ranged := strings.Cut(path[colon+1:], "-")
	first, err := strconv.Atoi(from)
	if err != nil || first <= 0 {
		return path, "", 0, 0
	}
	last = first
	if ranged {
		last, err = strconv.Atoi(to)
		if err != nil || last < first {
			return path, "", 0, 0
		}
	}
	return path[:colon], path[colon:], first, last
}
