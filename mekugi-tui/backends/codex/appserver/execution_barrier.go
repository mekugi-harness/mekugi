package appserver

// ExecutionBeforeToolParams is emitted by mekugi-app-server immediately before
// executing a local exec_command (Bash) or apply_patch call when the host opts
// into pre-tool capture observation.
type ExecutionBeforeToolParams struct {
	ThreadID string `json:"threadId"`
	TurnID   string `json:"turnId"`
	ItemID   string `json:"itemId"`
	Tool     string `json:"tool"`
	CWD      string `json:"cwd"`
	Input    string `json:"input"`
	SessionTmp string `json:"sessionTmp,omitempty"`
}

// ExecutionBeforeToolResponse acknowledges a pre-tool observation barrier.
type ExecutionBeforeToolResponse struct{}
