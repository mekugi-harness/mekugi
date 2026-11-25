// Source: internal/router/failure_store.go@[ac8511fa] retained failure publication and lookup.
package diagnostic

import (
	"encoding/json/jsontext"

	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
)

var Failures = recordkind.Spec{ID: "failure", Match: recordkind.HashedName("failure-", ".json")}

// Failure records retain the actual error for post-session diagnosis.
type Failure struct {
	Version   int            `json:"version"`
	Time      time.Time      `json:"time"`
	Thread    string         `json:"thread"`
	Phase     string         `json:"phase"`
	Code      string         `json:"code"`
	Reference string         `json:"reference"`
	Error     string         `json:"error,omitempty"`
	Stream    jsontext.Value `json:"stream,omitempty"`
}
