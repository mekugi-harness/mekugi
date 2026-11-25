package launch

import (
	"strings"

	"github.com/BurntSushi/toml"
	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
)

func appServerResumeConfig(args, resumeArgv []string) map[string]any {
	config := make(map[string]any)
	for i := 0; i < len(args); i++ {
		if args[i] != "-c" || i+1 == len(args) {
			continue
		}
		i++
		key, value, ok := strings.Cut(args[i], "=")
		key = strings.Trim(strings.TrimSpace(key), `"'`)
		if !ok || (key != "model" && key != "model_provider" && key != "model_reasoning_effort" && key != "service_tier" && key != "bypass_hook_trust") {
			continue
		}
		var parsed map[string]any
		if _, err := toml.Decode("value = "+value, &parsed); err == nil {
			config[key] = parsed["value"]
		} else {
			config[key] = strings.TrimSpace(value)
		}
	}
	if len(resumeArgv) > 0 {
		_, userArgs, err := launchoptions.SplitCommand(resumeArgv[1:])
		if err == nil && !launchoptions.HasModelOverride(userArgs) {
			delete(config, "model")
		}
	}
	return config
}
