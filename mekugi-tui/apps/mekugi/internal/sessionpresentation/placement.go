package sessionpresentation

import (
	"time"
)

// TurnPosition and ItemPosition retain only the identities and times needed to
// place history. Content and retained evidence stay with their feature owners.
type TurnPosition struct {
	ID      string
	Started time.Time
	Items   []ItemPosition
}

type ItemPosition struct {
	ID    string
	At    time.Time
	Known bool
}

type Placement struct {
	Turn, Anchor string
	At           time.Time
}
