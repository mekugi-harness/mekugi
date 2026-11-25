package preferences

import "time"

// Source: internal/router/app_server_panes.go:31:64@8bcb5cbf nativeAppliedSettings, nativeResumeEvidence

// Applied is a complete host-confirmed snapshot. Empty effort/tier select the
// host default; Model identifies the presence of the snapshot.
type Applied struct {
	Model    string    `json:"model"`
	Effort   string    `json:"effort"`
	Tier     string    `json:"tier"`
	Observed time.Time `json:"observed"`
}

// Evidence preserves partial history: older turn contexts do not identify tier.
type Evidence struct {
	Model     string `json:"model"`
	Effort    string `json:"effort"`
	Tier      string `json:"tier"`
	TierKnown bool   `json:"tierKnown"`
}
