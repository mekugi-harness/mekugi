package commandanalysis

// Source: internal/router/exec_observation.go:57:61,208:235,1656:1794@23344db6.
// Command is one stock command as the host will run it.
type Command struct {
	Command string
	Workdir string
	Shell   string `json:",omitempty"`
}
