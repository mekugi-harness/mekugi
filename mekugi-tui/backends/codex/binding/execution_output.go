package binding

import (
	jsonv1 "encoding/json"
	"strconv"
	"strings"
)

// ExecutionResult describes the host envelope, not the success of any nested
// tool. Consumers join native trace evidence before assigning nested outcomes.
type ExecutionResult struct {
	Terminal  bool
	Succeeded bool
	ExitCode  *int
	Text      string
}

// Source: internal/router/execution_continuation.go:200:309@b9a7b3fe.
// Match only the host's leading metadata, never strings printed by the program
// below Output:. Native session metadata inside exec needs a verified
// router carrier or transparent native-result projection as well.
func CodeModeExecutionHeader(text string) (status, cell string, body string) {
	first, rest, ok := strings.Cut(text, "\n")
	// Native output deduplication can label the host's short status part
	// when another part contains reusable text. Only accept the leading
	// label; references and status-like lines in program output are not facts.
	if number, labeled := strings.CutPrefix(first, "[call c"); labeled {
		number, valid := strings.CutSuffix(number, "]")
		if !valid || number == "" || number[0] == '0' || strings.Trim(number, "0123456789") != "" {
			return "", "", ""
		}
		first, rest, ok = strings.Cut(rest, "\n")
	}
	if !ok {
		return "", "", ""
	}
	if runningCell, running := strings.CutPrefix(first, "Script running with cell ID "); running {
		status, cell = "running", runningCell
	} else {
		base, suffix, more := strings.Cut(first, " (more output: wait with cell ID ")
		switch base {
		case "Script completed", "Script failed", "Script terminated":
			status = base
		default:
			return "", "", ""
		}
		if more {
			cell, ok = strings.CutSuffix(suffix, ")")
			if !ok || cell == "" || strings.ContainsAny(cell, "()") {
				return "", "", ""
			}
		}
	}
	if status == "running" && cell == "" || strings.ContainsAny(cell, " \t\r\n") {
		return "", "", ""
	}
	wall, rest, ok := strings.Cut(rest, "\n")
	// The native host can append its optional code-mode timing breakdown.
	if summary, timings, found := strings.Cut(wall, " (code-mode "); found {
		host, overhead, found := strings.Cut(timings, " seconds; overhead ")
		overhead, closed := strings.CutSuffix(overhead, " seconds)")
		if !found || !closed {
			return "", "", ""
		}
		if _, err := strconv.ParseFloat(host, 64); err != nil {
			return "", "", ""
		}
		if _, err := strconv.ParseFloat(overhead, 64); err != nil {
			return "", "", ""
		}
		wall = summary
	}
	if !ok || !strings.HasPrefix(wall, "Wall time ") || !strings.HasSuffix(wall, " seconds") {
		return "", "", ""
	}
	duration := strings.TrimSuffix(strings.TrimPrefix(wall, "Wall time "), " seconds")
	if _, err := strconv.ParseFloat(duration, 64); err != nil {
		return "", "", ""
	}
	if rest == "Output:" {
		return status, cell, ""
	}
	body, ok = strings.CutPrefix(rest, "Output:\n")
	if !ok {
		return "", "", ""
	}
	return status, cell, body
}

func NativeExecutionHeader(text string) (state, body string) {
	line, rest, ok := strings.Cut(text, "\n")
	if strings.HasPrefix(line, "Chunk ID: ") {
		line, rest, ok = strings.Cut(rest, "\n")
	}
	if !ok || !strings.HasPrefix(line, "Wall time: ") || !strings.HasSuffix(line, " seconds") {
		return "", ""
	}
	duration := strings.TrimSuffix(strings.TrimPrefix(line, "Wall time: "), " seconds")
	if _, err := strconv.ParseFloat(duration, 64); err != nil {
		return "", ""
	}
	state, rest, ok = strings.Cut(rest, "\n")
	if !ok {
		return "", ""
	}
	line, rest, ok = strings.Cut(rest, "\n")
	if after, ok0 := strings.CutPrefix(line, "Original token count: "); ok0 {
		if _, err := strconv.ParseUint(after, 10, 64); err != nil {
			return "", ""
		}
		line, rest, ok = strings.Cut(rest, "\n")
	}
	if !ok || (line != "Output:" && line != "Final output:") {
		return "", ""
	}
	return state, rest
}

func NativeExecutionSession(text string) int64 {
	state, _ := NativeExecutionHeader(text)
	id, running := strings.CutPrefix(state, "Process running with session ID ")
	if !running {
		return 0
	}
	value, err := strconv.ParseInt(id, 10, 64)
	if err != nil || value <= 0 {
		return 0
	}
	return value
}

func NativeJSONSession(text string) int64 {
	var result map[string]jsonv1.RawMessage
	if jsonv1.Unmarshal([]byte(text), &result) != nil {
		return 0
	}
	if batch, exists := result["results"]; exists {
		var results []map[string]jsonv1.RawMessage
		if jsonv1.Unmarshal(batch, &results) != nil || len(results) == 0 {
			return 0
		}
		// Completed prefixes have no live session. Only the current final
		// program can yield when a sequential carrier exits on a host error.
		result = results[len(results)-1]
	}
	var output string
	var id int64
	if jsonv1.Unmarshal(result["output"], &output) != nil ||
		jsonv1.Unmarshal(result["session_id"], &id) != nil || id <= 0 {
		return 0
	}
	if exit, exists := result["exit_code"]; exists && string(exit) != "null" {
		return 0
	}
	return id
}

// Source: apps/mekugi/internal/router/code_mode_failure.go@[c6bad96c].
// CodeModeFailureText reads the host's failure header and, when the host
// appended one as the final part, its complete script error. Presentation
// bounds the inline preview separately; the host result is never modified.
func CodeModeFailureText(texts []string) (string, bool) {
	if len(texts) == 0 {
		return "", false
	}
	if status, _, _ := CodeModeExecutionHeader(texts[0]); status != "Script failed" {
		return "", false
	}
	text := "exec script failed"
	if len(texts) > 1 {
		if detail, ok := strings.CutPrefix(texts[len(texts)-1], "Script error:\n"); ok {
			if strings.TrimSpace(detail) != "" {
				text += ": " + detail
			}
		}
	}
	return text, true
}
