package binding

// Source: internal/router/execution_continuation.go:31:40@[bb24aed2] annotation wire types.
// Source: internal/router/execution_continuation.go:165:265@[bb24aed2] execution evidence.
// Source: internal/router/execution_continuation.go:396:412@[bb24aed2] annotation recognition.

// Source: apps/mekugi/internal/router/native_tool_trace.go:51:57@[d428f9ae] nativeToolResult.

// ToolResult is host outcome evidence independent of model-printed text.
// Its retained field names preserve the historical change-record wire shape.
type ToolResult struct {
	CallID   string
	Tool     string
	Status   string
	ExitCode *int `json:",omitempty"`
}

// ExecutionNextCall is advice encoded in a historical continuation annotation,
// not a request to execute a tool.
type ExecutionNextCall struct {
	Tool  string `json:"tool"`
	Input any    `json:"input"`
}

// ExecutionContinuation retains the historical wire shape for offline inspection.
// New requests preserve stock Codex continuation guidance.
type ExecutionContinuation struct {
	Handle   map[string]any     `json:"handle"`
	NextCall *ExecutionNextCall `json:"next_call"`
	Reason   string             `json:"reason,omitempty"`
}

// RetainedExecution supplies validated carrier facts with a normalized kind.
// A nil value leaves classification to the original host input.
type RetainedExecution struct {
	CarrierKind string
	ToolName    string
	Script      string
}

// ExecutionCall describes recognized protocol evidence, not execution or success.
type ExecutionCall struct {
	CodeMode     bool
	Native       bool
	ResumeHandle string
}
