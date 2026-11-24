package executionobservation

const nativeExecCommandToolName = "exec_command"

// Carrier fields remain in retained history because observed stock calls
// may be custom or function tools. Mekugi no longer constructs a
// substitute execution carrier for stock editing or execution.
type codeModeCarrierKind string

const (
	codeModeCarrierCustom   codeModeCarrierKind = "custom"
	codeModeCarrierFunction codeModeCarrierKind = "function"
)
