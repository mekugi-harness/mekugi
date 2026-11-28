package presentation

// ReloadSource is an orchestration-issued attachment intent. Host events own its lifecycle.
type ReloadSource struct {
	Command, Path, Skill string
	Changes              string `json:",omitzero"`
	ExitCode             *int   // Present only after an authenticated per-source result.
	Preload              bool   `json:",omitzero"`
}
