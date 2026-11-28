package sessioncontrol

// ThreadFamily carries private invocation inputs only to native admission.
type ThreadFamily struct {
	TrackingCommand       string            `json:"trackingCommand,omitempty"`
	RequireCustomProvider bool              `json:"requireCustomProvider,omitempty"`
	Environment           map[string]string `json:"environment"`
	Profile               string            `json:"profile,omitempty"`
}

func ThreadPermissions(params map[string]any, approvals bool) map[string]any {
	if !approvals {
		params["approvalPolicy"], params["sandbox"] = "never", "danger-full-access"
	}
	return params
}
