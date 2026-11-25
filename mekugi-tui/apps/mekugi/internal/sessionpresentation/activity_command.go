package sessionpresentation

import (
	"cmp"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/shellsyntax"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/shellinstrumentation"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

// Source: apps/mekugi/internal/router/activity_command.go:1:76@45fbe279

// CommandPresentation admits native facts and existing output snapshots without
// transferring handles, capture authority, or process lifetime to presentation.
func CommandPresentation(entry ActivityEntry) activitypresentation.Command {
	c := activitypresentation.Command{Source: entry.Seq, Text: entry.Text, Observed: entry.Observed, Tail: entry.OutputTail, TailOmitted: entry.OutputOmit, TailClipped: entry.OutputClipped}
	n := entry.Native
	if n == nil {
		return c
	}
	c.UserShell = n.UserShell
	if n.Invocation != nil {
		canonical := n.Invocation.Snapshot()
		c.Script, c.Display, c.Directory = canonical.Script, canonical.Display, canonical.Directory
		c.Output, c.OutputContent = canonical.Output, len(n.Segments) > 0 && canonical.OutputContent
	} else {
		// Non-command items, including hooks, share the terminal detail surface.
		source := CommandSource(n.Command, n.CommandCwd)
		c.Script, c.Display, c.Directory = source.Script, cmp.Or(n.CommandDisplay, source.Display), source.Directory
		if n.Output != nil {
			snapshot, _ := n.Output.Context(0)
			c.Output = &snapshot
			c.OutputContent = len(n.Segments) > 0 && n.Output.HasContent()
		}
	}
	// Terminal publication can lag host completion while a burst rolls through.
	// Keep this overlay local; never turn the canonical invocation back to running.
	c.Started, c.Ended, c.Duration = n.CommandStarted, n.CommandEnded, n.Duration
	c.Running, c.Approval, c.Changes = n.Running, n.Approval, n.Changes
	if n.Exit != nil {
		c.Exited, c.ExitCode = true, *n.Exit
	}
	if receipt := n.CapturedEdit; receipt != nil {
		c.Captured = true
		if receipt.AnchorCall() == n.Item {
			c.CaptureText = receipt.Text
			if receipt.SourceAnchor.Call == n.Item {
				c.CaptureAfter = receipt.SourceAnchor.Segment
			}
		}
	}
	for _, segment := range n.Segments {
		if n.SegmentsPending {
			break
		}
		segmentDisplay := frontends.ShellDisplaySource(segment.Source)
		if len(n.Segments) == 1 && c.Display != "" && strings.TrimRight(c.Display, "\r\n") != strings.TrimRight(segmentDisplay, "\r\n") {
			if rest, _, ok := shellsyntax.StripLeadingCd(segmentDisplay, cmp.Or(n.CommandCwd, c.Directory)); ok && strings.TrimRight(rest, "\r\n") == strings.TrimRight(c.Display, "\r\n") {
				segmentDisplay = rest
			}
		}
		value := activitypresentation.CommandSegment{
			Source: segment.Source, Display: segmentDisplay, Text: segment.Text, Running: segment.Running, Skipped: segment.Skipped, Pending: segment.Pending, ExitCode: segment.Exit,
			Started: segment.Timing.Started, Ended: segment.Timing.Ended, Duration: time.Duration(segment.Timing.ElapsedNS),
			Tail: segment.Tail, TailOmitted: segment.Omit, TailClipped: segment.Clipped, Changes: segment.Changes, Results: segment.Results,
		}
		if c.Captured && !segment.Pending && !segment.Running && !segment.Skipped && segment.Exit == 0 {
			// One observed operation in one captured invocation needs no source
			// guess to replace its Run row. Grouped calls still keep neighbors.
			value.Writer = len(n.CapturedEdit.Calls) == 1 && len(n.Segments) == 1 ||
				(activitypresentation.CommandSource{Script: segment.Source, Display: value.Display, Directory: ""}).WritesFilesIn(c.Script)
		}
		if segment.Output != nil {
			snapshot, _ := segment.Output.Context(0)
			value.Output = &snapshot
			value.OutputContent = segment.Output.HasContent()
		}
		c.Segments = append(c.Segments, value)
	}
	return c
}

func CommandSource(command, directory string) activitypresentation.CommandSource {
	script, ok := shellinstrumentation.CommandScript(command)
	if !ok {
		script = shellinstrumentation.ShOriginal(command)
	}
	return activitypresentation.CommandSource{Script: script, Display: frontends.ShellDisplaySource(DisplayCommand(command)), Directory: directory}
}

func itemCommandSource(item observation.Item, directory string) activitypresentation.CommandSource {
	source := CommandSource(item.Command, directory)
	source.Display = CommandDisplaySource(item)
	return source
}
