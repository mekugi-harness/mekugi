package sessionpresentation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
)

type InterruptIntent struct {
	Version Version
	Target  sessioncontrol.InterruptTarget
	Mode    sessioncontrol.InterruptMode
}
