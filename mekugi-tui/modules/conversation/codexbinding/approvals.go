// Source: apps/mekugi/internal/router/app_server_approvals.go:280:370@[42afca9c] native approval mapping.
package codexbinding

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
)

type Approval struct {
	Record         prompt.Approval
	Responses      []map[string]any
	DisplayCommand bool
}
