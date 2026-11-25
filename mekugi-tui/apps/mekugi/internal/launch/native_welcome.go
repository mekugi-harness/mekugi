package launch

import (
	"runtime/debug"
)

// Release builds set this to their tag with -ldflags -X.
var buildVersion string

// Read the running executable's build identity, never the workspace checkout.
func mekugiVersion() string {
	if buildVersion != "" {
		return buildVersion
	}
	if info, ok := debug.ReadBuildInfo(); ok {
		for _, setting := range info.Settings {
			if setting.Key == "vcs.revision" && setting.Value != "" {
				return setting.Value[:min(8, len(setting.Value))]
			}
		}
		if info.Main.Version != "" && info.Main.Version != "(devel)" {
			return info.Main.Version
		}
	}
	return "dev"
}
