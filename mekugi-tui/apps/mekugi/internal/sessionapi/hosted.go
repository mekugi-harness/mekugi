package sessionapi

type StartOptions struct {
	Workspaces   []string     `json:"workspaces"`
	Models       []ModelGroup `json:"models"`
	DefaultModel string       `json:"defaultModel"`
	Note         string       `json:"note,omitzero"`
}
type ModelGroup struct {
	Label  string  `json:"label"`
	Models []Model `json:"models"`
}
type Model struct {
	ID            string   `json:"id"`
	Label         string   `json:"label"`
	Efforts       []string `json:"efforts"`
	DefaultEffort string   `json:"defaultEffort,omitzero"`
}
type Start struct {
	Native    *NativeStart `json:"-"`
	Replay    *ReplayStart `json:"replay,omitzero"`
	Workspace string       `json:"workspace"`
	Model     string       `json:"model"`
	Effort    string       `json:"effort,omitzero"`
	Resume    string       `json:"resume,omitzero"`
}

// ReplayStart selects retained history, never live input or a client workspace.
type ReplayStart struct {
	Session           string   `json:"session"`
	Speed             *float64 `json:"speed,omitzero"`
	Seed              *uint64  `json:"seed,omitzero"`
	ChildReplySummary *bool    `json:"childReplySummary,omitzero"`
}
