package binding

type ShellGuardKind string

const (
	RemoteWrite ShellGuardKind = "remoteWrite"
	Sudo        ShellGuardKind = "sudo"
)

type ShellGuardSecretKind string

const (
	SudoPassword  ShellGuardSecretKind = "sudoPassword"
	GpgPassphrase ShellGuardSecretKind = "gpgPassphrase"
)

// ShellGuardApprovalParams decodes native v2 `shellGuard/requestApproval` server requests.
type ShellGuardApprovalParams struct {
	ThreadID    string         `json:"threadId"`
	TurnID      string         `json:"turnId"`
	ItemID      string         `json:"itemId"`
	StartedAtMs int64          `json:"startedAtMs"`
	Kind        ShellGuardKind `json:"kind"`
	Cwd         string         `json:"cwd"`
	Executable  string         `json:"executable"`
	Argv        []string       `json:"argv"`
}

// ShellGuardApprovalResponse encodes client responses to `shellGuard/requestApproval`.
type ShellGuardApprovalResponse struct {
	Decision string  `json:"decision"`
	Reason   *string `json:"reason"`
}

// ShellGuardSecretParams decodes native v2 `shellGuard/requestSecret` server requests.
type ShellGuardSecretParams struct {
	ThreadID    string               `json:"threadId"`
	TurnID      string               `json:"turnId"`
	ItemID      string               `json:"itemId"`
	StartedAtMs int64                `json:"startedAtMs"`
	Kind        ShellGuardSecretKind `json:"kind"`
	Cwd         string               `json:"cwd"`
	Executable  string               `json:"executable"`
	Argv        []string             `json:"argv"`
}

// ShellGuardSecretResponse encodes client responses to `shellGuard/requestSecret`.
type ShellGuardSecretResponse struct {
	Decision string  `json:"decision"`
	Secret   *string `json:"secret"`
	Reason   *string `json:"reason"`
}
