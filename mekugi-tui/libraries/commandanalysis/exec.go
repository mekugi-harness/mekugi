package commandanalysis

// MaxScriptBytes bounds static observation of a stock JavaScript cell.
const MaxScriptBytes = 1 << 20

// Source: internal/router/exec_observation.go:57:61,208:235,1656:1794@23344db6.
// Command is one stock command as the host will run it.
type Command struct {
	Command string
	Workdir string
	Shell   string `json:",omitempty"`
}

func LocalEnvironment(environmentID []byte) bool {
	return len(environmentID) == 0 || string(environmentID) == "null"
}
