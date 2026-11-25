package sessionapi

import (
	"context"

	"encoding/json/jsontext"
)

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

func (s *Server) BindProfileVerifier(verify func(context.Context, string, ProfileIdentity) error) {
	s.mu.Lock()
	s.verifyProfile = verify
	s.mu.Unlock()
}

func (h *Hub) startNative(requestContext, viewContext context.Context, body jsontext.Value) (*rpcSubscription, *RPCError) {
	var native NativeStart
	if err := decodeBody(body, &native); err != nil {
		return nil, err
	}
	_, subscription, err := h.admitRuntime(requestContext, viewContext, Start{Workspace: native.Workspace, Native: &native})
	if err != nil {
		return nil, runtimeStartFailure(err)
	}
	return subscription, nil
}
