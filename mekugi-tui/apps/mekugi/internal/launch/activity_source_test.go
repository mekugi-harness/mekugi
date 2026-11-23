package launch

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/sourceanalysis/markdownsyntax"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

func toolActivityShell(source string) string {
	return activitypresentation.ShellSource(frontends.ShellDisplaySource(source), "")
}
func toolActivityFenced(language, source string) string {
	return markdownsyntax.SourceFence(language, source)
}
