// Source: internal/vcsguard/hook.go:18:106@[2ba2151a] native hook protocol.
package shellhook

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandargs"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/shellinstrumentation"
)

const Flag = "--tracking-hook"

// Input is the native hook event. Trusted tracking resources come from argv.
type Input struct {
	Event      string `json:"hook_event_name"`
	Tool       string `json:"tool_name"`
	Item       string `json:"tool_use_id"`
	Thread     string `json:"session_id"`
	Turn       string `json:"turn_id"`
	CWD        string `json:"cwd"`
	SessionTmp string `json:"mekugi_session_tmp"`
	Input      struct {
		Command *string `json:"command"`
	} `json:"tool_input"`
}

// TrackingCommand builds the optional observer invoked after caller hooks.
func TrackingCommand(helper, tracker string) string {
	arguments := []string{helper, Flag, tracker}
	for i := range arguments {
		arguments[i] = commandargs.Quote(arguments[i])
	}
	return "/bin/sh -c " + commandargs.Quote(strings.Join(arguments, " ")+" || :")
}

// Rewrite instruments tracked sh commands.
func Rewrite(request Input, tracker string) (any, error) {
	if request.Event != "PreToolUse" || request.Tool != "Bash" || request.Input.Command == nil {
		return nil, fmt.Errorf("unexpected hook input")
	}
	if tracker != "" && !filepath.IsAbs(tracker) {
		return nil, fmt.Errorf("hook tracker must be absolute")
	}
	changed := *request.Input.Command
	if tracker != "" {
		if info, err := os.Stat(tracker); err == nil && info.Mode().IsRegular() {
			changed = shellinstrumentation.ShScript(tracker, changed)
		}
	}
	var response any = map[string]any{}
	if changed != *request.Input.Command {
		response = map[string]any{"hookSpecificOutput": map[string]any{
			"hookEventName": "PreToolUse", "permissionDecision": "allow",
			"updatedInput": map[string]string{"command": changed},
		}}
	}
	return response, nil
}
