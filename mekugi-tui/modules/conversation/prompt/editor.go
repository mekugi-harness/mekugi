package prompt

import "github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"

// Editor parks Main's editor while a question or denial reason owns input.
// The renderer keeps geometry and its preferred cursor column separately.
type Editor struct{ Parked composer.State }
