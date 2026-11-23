package launch

import (
	"os"

	launchoptions "github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/launch/options"
)

func skillsManagerInPath(environment []string, workspace, extraDirectory string) bool {
	path := launchoptions.EnvironmentValue(withoutInheritedFrontend(environment), "PATH")
	if extraDirectory != "" {
		path = extraDirectory + string(os.PathListSeparator) + path
	}
	_, err := invocationExecutable("skills-mgr", []string{"PATH=" + path}, workspace)
	return err == nil
}
