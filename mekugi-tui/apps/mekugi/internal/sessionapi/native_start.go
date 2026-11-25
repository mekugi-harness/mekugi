package sessionapi

// NativeStart preserves native invocation arguments on the private socket only.
// A startup operation is not a thread identity. It hosts welcome/resume selection
// before the execution owner confirms a logical thread.
type NativeStart struct {
	Workspace   string   `json:"workspace"`
	RouterArgs  []string `json:"routerArgs"`
	Args        []string `json:"args"`
	Environment []string `json:"environment,omitzero"`
}

// ProfileIdentity is private attachment admission, never a public session fact.
type ProfileIdentity struct {
	Name string `json:"name"`
	Home string `json:"home"`
}
