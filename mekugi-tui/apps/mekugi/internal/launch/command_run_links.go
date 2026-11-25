package launch

import (
	"regexp"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

var (
	commandRunID    = regexp.MustCompile(`^exec-[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$`)
	omittedRunBytes = regexp.MustCompile(`^\.\.\. [0-9]+ bytes omitted \.\.\.$`)
)

func commandRunTarget(target string) (id string, first, last int, ok bool) {
	ref, found := strings.CutPrefix(target, "mekugi:run/")
	if !found {
		return "", 0, 0, false
	}
	id, _, first, last = markdownFileLocation(ref)
	return id, first, last, commandRunID.MatchString(id)
}

func commandRunLink(target string) bool {
	_, _, _, ok := commandRunTarget(target)
	return ok
}

func commandRunPages(source *sessionpresentation.ActivitySource, thread, id string) []activity.DetailPage {
	if source == nil {
		return nil
	}
	for _, entry := range source.Entries {
		item := entry.Source.Native
		if item == nil || item.Thread != thread || item.Item != id || item.Command == "" {
			continue
		}
		// Segment and source pages need not expose the combined command output.
		// A run link selects that output directly, without retaining another copy.
		pages := activity.DetailPages([]activity.Value{{
			Source: entry.Source.Seq, Kind: "op", Verb: "Run", Label: "Command output",
			Code: sessionpresentation.CommandPresentation(entry.Source).Display, Lang: "bash", Output: 1,
			Tail: entry.Source.OutputTail, TailOmitted: entry.Source.OutputOmit,
			TailClipped: entry.Source.OutputClipped, Running: item.Running,
		}})
		pages[0].CombinedOutput = true
		return pages
	}
	return nil
}

func runRangeAvailable(output feed.OutputView, first, last int) bool {
	if !output.Done || output.Released || output.Truncated || output.Dropped != 0 || first < 1 || last < first || last > len(output.Lines) {
		return false
	}
	// Unified exec can omit the middle of the aggregate before retention sees
	// it. A line number after that marker is not a full-run coordinate.
	for _, line := range output.Lines[:last] {
		if omittedRunBytes.MatchString(line) {
			return false
		}
	}
	return true
}
