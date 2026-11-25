package launch

import (
	"regexp"
	"strings"
)

var (
	commandRunID = regexp.MustCompile(`^exec-[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$`)
)

func commandRunTarget(target string) (id string, first, last int, ok bool) {
	ref, found := strings.CutPrefix(target, "mekugi:run/")
	if !found {
		return "", 0, 0, false
	}
	id, _, first, last = markdownLinkLocation(ref)
	return id, first, last, commandRunID.MatchString(id)
}

func commandRunLink(target string) bool {
	_, _, _, ok := commandRunTarget(target)
	return ok
}
