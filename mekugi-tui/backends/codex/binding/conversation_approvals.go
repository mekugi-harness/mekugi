// Source: apps/mekugi/internal/router/app_server_approvals.go:265:500@[42afca9c] Codex approval protocol and stock choice semantics.
package binding

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
)

type ApprovalDecision struct {
	Label                  string
	Response               map[string]any
	Approve, Session, Deny bool
	// Lasting decisions outlast this request, for the session or in policy.
	Lasting bool
	Outcome string
}
type ApprovalRequest struct {
	Kind           string `json:"kind"`
	ThreadID       string `json:"threadId"`
	TurnID         string `json:"turnId"`
	ItemID         string `json:"itemId"`
	Reason         string `json:"reason"`
	Command        string `json:"command"`
	Cwd            string `json:"cwd"`
	GrantRoot      string `json:"grantRoot"`
	NetworkContext *struct {
		Host string `json:"host"`
	} `json:"networkApprovalContext"`
	Additional         jsontext.Value   `json:"additionalPermissions"`
	Amendment          []string         `json:"proposedExecpolicyAmendment"`
	Permissions        jsontext.Value   `json:"permissions"`
	AvailableDecisions []jsontext.Value `json:"availableDecisions"`
}

type ApprovalReview struct {
	Thread, Turn, Item, Status string
}

func DecodeApprovalRequest(params jsontext.Value) (ApprovalRequest, error) {
	var p ApprovalRequest
	err := json.Unmarshal(params, &p)
	return p, err
}
