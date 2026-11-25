package sessionpresentation

// Source: apps/mekugi/internal/router/app_server_events.go:484:578@7166de95;
// app_server_search.go:15:52, activity_vcs.go:13:20, live_activity_blocks.go:9:14.
import (
	"cmp"
	"path/filepath"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/shellsyntax"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/pathdisplay"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/sourceanalysis/markdownsyntax"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

// cdFailed reports whether a command's leading output line is an outer-shell
// diagnostic for the leading cd target itself (including zero-exit semicolon
// or newline lists such as "cd missing; pwd"), without matching child script
// cd errors inside the entered directory.
func cdFailed(item observation.Item, display string) bool {
	if item.AggregatedOutput == nil {
		return false
	}
	operand, ok := shellsyntax.LeadingCdOperand(display)
	if !ok {
		return false
	}
	for line := range strings.SplitSeq(*item.AggregatedOutput, "\n") {
		if line = strings.TrimSpace(line); line != "" {
			return isOuterShellCdError(line, operand)
		}
	}
	return false
}

func isOuterShellCdError(line, operand string) bool {
	if after, ok := strings.CutPrefix(line, "cd: "); ok {
		return strings.Contains(after, operand)
	}
	prefix, rest, ok := strings.Cut(line, ":")
	if !ok {
		return false
	}
	switch shellsyntax.InterpreterIdentity(prefix) {
	case "bash", "sh", "dash", "zsh", "ash", "ksh":
	default:
		return false
	}
	if _, after, found := strings.Cut(rest, ": cd: "); found {
		return strings.Contains(after, operand)
	}
	if after, found := strings.CutPrefix(rest, " cd: "); found {
		return strings.Contains(after, operand)
	}
	if after, found := strings.CutPrefix(rest, "cd:"); found {
		return strings.Contains(after, operand)
	}
	return false
}

func effectiveItemCommand(item observation.Item) (display, dir string, stripped bool) {
	display = frontends.ShellDisplaySource(DisplayCommand(item.Command))
	if !cdFailed(item, display) {
		if rest, target, ok := shellsyntax.StripLeadingCd(display, item.Cwd); ok {
			return rest, target, true
		}
	}
	return display, item.Cwd, false
}

// SingleSegmentDisplay applies a command's leading-cd omission to a single
// tracked segment source when it matches expectedDisplay (ignoring trailing
// newlines trimmed by shellinstrumentation.Split).
func SingleSegmentDisplay(source, expectedDisplay, dir string) string {
	display := frontends.ShellDisplaySource(source)
	trimmedExpected := strings.TrimRight(expectedDisplay, "\r\n")
	if trimmedExpected != "" && trimmedExpected != strings.TrimRight(display, "\r\n") {
		if rest, _, ok := shellsyntax.StripLeadingCd(display, dir); ok && strings.TrimRight(rest, "\r\n") == trimmedExpected {
			return rest
		}
	}
	return display
}
// CommandDisplaySource returns the display shell source for a command item,
// omitting a safe leading literal cd <dir> && prefix when its directory can be
// resolved.
func CommandDisplaySource(item observation.Item) string {
	display, _, _ := effectiveItemCommand(item)
	return display
}

// CommandWorkdir is the display form of the directory the host ran
// a command in, when it is not the workspace its activity is shown against.
// An unknown workspace or directory gets no label.
func CommandWorkdir(item observation.Item, workspace string) string {
	if item.Type != "commandExecution" || !filepath.IsAbs(workspace) {
		return ""
	}
	dir := item.Cwd
	if item.Command != "" {
		if _, target, stripped := effectiveItemCommand(item); stripped {
			dir = target
		}
	}
	if !filepath.IsAbs(dir) || filepath.Clean(dir) == filepath.Clean(workspace) {
		return ""
	}
	return pathdisplay.ForWorkspace(workspace, filepath.Clean(dir))
}

func CommandDirectory(item observation.Item, workspace string) string {
	if (item.Type == "" || item.Type == "commandExecution") && item.Command != "" {
		if _, dir, stripped := effectiveItemCommand(item); stripped {
			return dir
		}
	}
	if filepath.IsAbs(item.Cwd) {
		return item.Cwd
	}
	return workspace
}

// CommandText uses Codex's typed classification, then the shared
// display classifier for frontends that Codex does not recognize. Neither
// classification changes the executed command or retained host item. Paths
// display relative to the directory the command ran in, which
// CommandWorkdir labels when it differs from the workspace.
func CommandText(item observation.Item, cwd string) string {
	displayCmd, effectiveDir, stripped := effectiveItemCommand(item)
	baseCwd := item.Cwd
	if !filepath.IsAbs(baseCwd) {
		baseCwd = cwd
	}
	displayCwd := effectiveDir
	if !filepath.IsAbs(displayCwd) {
		displayCwd = cwd
	}
	// The host rebases read action paths against the starting directory,
	// while search and list operands stay relative to the effective directory.
	readActionPath := func(path string) string {
		if stripped && path != "" && !filepath.IsAbs(path) && filepath.IsAbs(baseCwd) {
			path = filepath.Join(baseCwd, path)
		}
		return pathdisplay.ForWorkspace(displayCwd, path)
	}
	var parts []string
	for _, action := range item.CommandActions {
		switch action.Type {
		case "read":
			parts = append(parts, "Read "+markdownsyntax.QuoteCode(readActionPath(action.Path)))
		case "listFiles":
			parts = append(parts, "List "+markdownsyntax.QuoteCode(cmp.Or(pathdisplay.ForWorkspace(displayCwd, action.Path), ".")))
		case "search":
			text := "Search " + markdownsyntax.QuoteCode(action.Query)
			if action.Path != "" {
				text += " in " + markdownsyntax.QuoteCode(pathdisplay.ForWorkspace(displayCwd, action.Path))
			}
			parts = append(parts, text)
		default:
			parts = nil
		}
		if parts == nil {
			break
		}
	}
	if len(parts) == 0 {
		return activitypresentation.ShellSource(displayCmd, displayCwd)
	}
	return strings.Join(parts, "\n\n")
}

// completeCommand adapts a host result into the shared invocation owner before
// terminal or report settlement. History has no revived start/end clock.
func (s *Attachment) CompleteCommand(native *NativeItem, item observation.Item, outputs *feed.Retention) {
	if item.Type != "commandExecution" {
		return
	}
	native.UserShell = item.UserShell
	native.Exit = item.ExitCode // Displayed outcome, separate from queued visual completion.
	if run := s.Commands()[[3]string{native.Thread, native.Turn, native.Item}]; run != nil {
		native.Invocation = run.Entry.Native.Invocation
	}
	source := itemCommandSource(item, native.CommandCwd)
	if native.Invocation == nil {
		native.Invocation = activitypresentation.NewInvocation(source, native.CommandStarted, outputs)
	}
	native.Invocation.Complete(activitypresentation.CommandCompletion{
		Source: source, At: native.CommandEnded, Duration: ItemDuration(item), Aggregate: item.AggregatedOutput, Exit: item.ExitCode,
	})
	native.Output = native.Invocation.Output()
}

// CommandOutput keeps aggregate output even when the host reports no
// exit. Known failures carry their tail on the separate exit entry. Only confirmed
// success opens the settlement window or starts restored output collapsed.
func CommandOutput(entry *ActivityEntry, item observation.Item, settled time.Time) {
	if item.Type != "commandExecution" || !item.UserShell && item.ExitCode != nil && *item.ExitCode != 0 {
		return
	}
	entry.OutputTail, entry.OutputOmit, entry.OutputClipped = OutputTail(item.AggregatedOutput)
	if item.ExitCode == nil || *item.ExitCode != 0 {
		return
	}
	entry.Completed = settled
	entry.Native.Changes, entry.Native.Commit = CommandOutputRows(item)
}

// OutputTail keeps the last non-blank lines of a command's host
// output for display. It never reads beyond the host's aggregate, and counts
// the lines it leaves out.
func OutputTail(output *string) ([]string, int, bool) {
	if output == nil {
		return nil, 0, false
	}
	return feed.TailOutput(*output)
}

func ToolText(item observation.Item, cwd string) string {
	if item.Type == "mcpToolCall" {
		return activitypresentation.MCPText(item)
	}
	if item.Type != "webSearch" {
		return CommandText(item, cwd)
	}
	query := item.Query
	if action := item.Action; action != nil {
		switch action.Type {
		case "openPage":
			return "Open " + markdownsyntax.QuoteCode(action.URL)
		case "findInPage":
			return "Search " + markdownsyntax.QuoteCode(action.Pattern) + " in " + markdownsyntax.QuoteCode(action.URL)
		case "search":
			query = cmp.Or(strings.Join(action.Queries, " | "), action.Query, query)
		}
	}
	return "Web Search " + markdownsyntax.QuoteCode(query)
}

// Count returned records only when host evidence is complete and attributable
// to one search. Missing output, errors, context rows and truncated output are
// not zero results. No command is executed to infer a count.
func SearchResults(item observation.Item) *int {
	if item.Type == "webSearch" {
		if item.Results != nil {
			return item.Results
		}
		return nil
	}
	if item.Type != "commandExecution" || item.Status != "completed" || item.ExitCode == nil || *item.ExitCode < 0 || *item.ExitCode > 1 {
		return nil
	}
	// Codex's shell aggregate can silently cap at 1 MiB.
	// Source: codex-rs/core/src/exec.rs:865:910 (DEFAULT_OUTPUT_BYTES_CAP).
	if item.AggregatedOutput != nil && len(*item.AggregatedOutput) >= 1<<20 {
		return nil
	}
	displayCmd, _, _ := effectiveItemCommand(item)
	return activitypresentation.OperationResultCount(displayCmd, item.AggregatedOutput, *item.ExitCode)
}

func ItemDuration(item observation.Item) time.Duration {
	if item.DurationMS == nil {
		return 0
	}
	return time.Duration(*item.DurationMS) * time.Millisecond
}

// CommandOutputRows adapts observed change-history or VCS output, retaining the
// immutable commit key whose files can complete the output-observed rows.
func CommandOutputRows(item observation.Item) ([]activitypresentation.ChangeRow, activitypresentation.CommitKey) {
	if item.Status == "failed" || item.Status == "interrupted" || item.ExitCode == nil || *item.ExitCode != 0 {
		return nil, activitypresentation.CommitKey{}
	}
	displayCmd, effectiveDir, _ := effectiveItemCommand(item)
	if rows := activitypresentation.MChangesOutputRows(displayCmd, item.AggregatedOutput); rows != nil || item.AggregatedOutput == nil {
		return rows, activitypresentation.CommitKey{}
	}
	return activitypresentation.VCSOutputRows(displayCmd, effectiveDir, *item.AggregatedOutput)
}
