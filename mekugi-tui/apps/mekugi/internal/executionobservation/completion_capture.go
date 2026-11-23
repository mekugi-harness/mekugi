package executionobservation

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandargs"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/attachments"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/agentguidance/compact"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"
)

// CompletionTokens bounds one automatic attachment, including source labels,
// diagnostics and its continuation. Explicit preloads and reloads remain full.
const CompletionTokens = 4000

const approxBytesPerToken = 4

// approxTokenCount matches codex-utils-string::approx_token_count (ceil(bytes/4)).
func approxTokenCount(s string) int {
	return (len(s) + approxBytesPerToken - 1) / approxBytesPerToken
}

func truncateUTF8Prefix(s string, maxBytes int) string {
	if maxBytes <= 0 {
		return ""
	}
	if len(s) <= maxBytes {
		return s
	}
	end := maxBytes
	for end > 0 && !utf8.RuneStart(s[end]) {
		end--
	}
	return s[:end]
}

type completionSource struct {
	command    string
	start, end int
}

// Frame only complete, visible diff rows. Leave the raw prefix and retained
// suffix in their original representation, including paging diagnostics.
func completionText(result compact.CapturedSource, sources []completionSource) string {
	var text strings.Builder
	var bodies []attachments.SourceRange
	through := 0
	for _, source := range sources {
		if source.start >= len(result.Stdout) {
			break
		}
		end := source.end
		if end > len(result.Stdout) {
			end = source.start + strings.LastIndexByte(result.Stdout[source.start:], '\n') + 1
		}
		if end <= source.start {
			break
		}
		text.WriteString(result.Stdout[through:source.start])
		fmt.Fprintf(&text, "Source %q:\n", source.command)
		start := text.Len()
		text.WriteString(result.Stdout[source.start:end])
		bodies = append(bodies, attachments.SourceRange{Start: start, End: text.Len()})
		through = end
	}
	if len(bodies) == 0 {
		return result.Text()
	}
	text.WriteString(result.Stdout[through:])
	result.Stdout = text.String()
	return attachments.SourceRanges(result.Text(), bodies)
}

func boundCompletion(ctx context.Context, result compact.CapturedSource, sources []completionSource, selected changes.Scope) (string, error) {
	// Avoid copying a full retained diff just to reject it on the byte limit.
	if len(result.Stdout) <= approxBytesPerToken*CompletionTokens {
		text := completionText(result, sources)
		if approxTokenCount(text) <= CompletionTokens {
			return text, nil
		}
	}
	if selected.Store == nil {
		return "", errors.New("completion retained storage is unavailable")
	}
	full := result.Stdout
	path, _, err := selected.Store.WriteReadOnlyFile(ctx, selected.Thread, selected.Limits, selected.Writes, "completion", ".txt", []byte(full))
	if err != nil {
		return "", err
	}
	quotedPath := commandargs.Quote(path)
	totalLines := strings.Count(full, "\n")
	if full != "" && !strings.HasSuffix(full, "\n") {
		totalLines++
	}
	completionNotice := func(prefix string) string {
		startLine := strings.Count(prefix, "\n") + 1
		if startLine > 1 && startLine <= totalLines {
			return fmt.Sprintf("Completion output exceeds budget. Read the remaining output with: mcat %s %d:%d\n", quotedPath, startLine, totalLines)
		}
		return "Completion output exceeds budget. Read the complete output with: mcat " + quotedPath + "\n"
	}
	maxNotice := fmt.Sprintf("Completion output exceeds budget. Read the remaining output with: mcat %s %d:%d\n", quotedPath, totalLines, totalLines)
	remaining := CompletionTokens - approxTokenCount("\n"+maxNotice)
	if remaining <= 0 {
		return "", errors.New("completion continuation exceeds output budget")
	}
	var text string
	for {
		prefix := truncateUTF8Prefix(full, remaining*approxBytesPerToken)
		result.Stdout = prefix
		result.Stderr = completionNotice(prefix)
		text = completionText(result, sources)
		tokens := approxTokenCount(text)
		if tokens <= CompletionTokens {
			break
		}
		remaining -= tokens - CompletionTokens
		if remaining <= 0 {
			return "", errors.New("completion continuation exceeds output budget")
		}
	}
	return text, nil
}
