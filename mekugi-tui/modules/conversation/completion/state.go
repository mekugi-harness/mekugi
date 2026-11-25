// Package completion owns draft targets and query results, without search effects.
// Source: apps/mekugi/internal/router/app_server_picker.go:24:74,163:182,398:405
// and app_server_skills.go:70:96,204:210@[0a356401] composer completion state.
package completion

type Target struct {
	Kind       byte
	Start, End int
	Query      string
}

type Choice struct {
	ID                               uint64 // Distinguishes repeated labels in one immutable choice collection.
	Name, Path, Description, Display string
	Enabled                          bool
	Preset                           bool
	// Current marks the active value of a setting; Unavailable marks one the
	// host refuses to select now.
	Current, Unavailable bool
	// Danger marks a choice that stops work, discards data or removes a
	// safeguard. Renderers tone it and require deliberate confirmation.
	Danger     bool
	HideRemote bool `json:"-"` // A dedicated browser control replaces this completion.
}

type ChoiceID struct {
	Name, Path string
	ID         uint64
}

func (c Choice) Identity() ChoiceID { return ChoiceID{Name: c.Name, Path: c.Path, ID: c.ID} }

func (c Choice) Label() string {
	if c.Display != "" {
		return c.Display
	}
	return c.Name
}

// State belongs to the attached composer. Pending retains query identity only;
// the application owns requests, scan cancellation, and skill-write correlation.
type State struct {
	CopyTexts         []string
	Target, Dismissed Target
	Open              bool
	Choices           []Choice
	Loading           bool
	Problem           string
	Pending           *Target
	PendingCwd        string
	Resolved          Target
	ResolvedCwd       string
	Skills            []Choice
	Presets           []Choice
	SkillsCwd         string
	SkillsLoaded      bool
	SkillsProblem     string
	Query             string
	Initial           map[string]bool
}

type View struct {
	Target         Target
	Choices        []Choice
	Loading        bool
	Problem, Query string
}
